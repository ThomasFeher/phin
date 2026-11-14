//! The category tree, built from `categories.csv` content given as a string.
//!
//! The contract file has two columns, `parent;name`, where `parent` is the NAME
//! of the parent node or empty for a root node, and `name` is the node's own
//! name. A category is addressed by the `parent:child` path built by chaining
//! immediate parents, so the rows `finance;expense` and `expense;transport`
//! make the three-level path `finance:expense:transport` valid. Node names are
//! therefore unique across the whole tree: a `parent` column names a node, not
//! a path, so the same name under two different parents (or twice in the file)
//! would make that parent reference ambiguous. That is the stricter reading the
//! plan leaves open, and it is a typed error, [`BuildError.DuplicateName`],
//! naming the duplicated node — silently keeping the first row would let one
//! row's parent quietly shadow another's.
//!
//! File order is display order and is preserved: [`roots`] and [`children`]
//! return names in the order the rows appear in the text.
//!
//! Only the simple two-column form is parsed here: category names contain no
//! `;`, so a row is split on the separator and must yield exactly two columns.
//! Quoting and the rest of the CSV codec are plan item 2.1 in the technical
//! layer, which owns reading the file; this module only ever sees its content
//! as a `&str`.

/// Reason the `categories.csv` text could not become a tree.
#[derive(Debug, Eq, PartialEq)]
pub enum BuildError {
    /// A content line did not have exactly two columns with a non-empty name;
    /// the payload is the 1-based line number in the text.
    MalformedRow(usize),
    /// A `parent` column named a node that no row defines; the payload is that
    /// parent name.
    MissingParent(String),
    /// The same node name was defined twice, which makes the `parent` column
    /// ambiguous; the payload is the duplicated name.
    DuplicateName(String),
    /// The `parent` relations contain a cycle, e.g. `a;b` next to `b;a`, or a
    /// self-parent `a;a`; the payload is a node on the cycle. Detection walks
    /// each node at most twice, so a cycle is an error, never a hang.
    Cycle(String),
}

/// Reason a `parent:child` path does not name a node of the tree.
#[derive(Debug, Eq, PartialEq)]
pub enum PathError {
    /// The empty path, which names nothing.
    EmptyPath,
    /// A path with an empty segment, e.g. `food:` or `food::supermarket`.
    EmptySegment,
    /// A segment no node matches; the payload is the offending segment and the
    /// names that were available at that point in the path: the root names for
    /// the first segment, the child names for a later one. The candidate list
    /// is never empty while the tree has a name to name, so a message can
    /// always suggest something the caller could have meant: when the failing
    /// segment follows a leaf, whose child list would be empty, the payload
    /// names the leaf itself and the leaf's siblings instead. Only a path into
    /// a tree that holds no node at all can carry an empty list, since there
    /// is then no name to suggest.
    UnknownCategory(String, Vec::<String>),
}

/// The category tree: node names in file order, each with its parent index
/// (`NO_PARENT` for a root) and its child indices in file order.
#[derive(Debug)]
pub struct Tree {
    names: Vec::<String>,
    parent: Vec::<usize>,
    children: Vec::<Vec::<usize>>,
}

const NO_PARENT: usize = usize::MAX;

/// Index of `name` in `names`, or `NO_PARENT` when absent.
fn index_of(names: &Vec::<String>, name: &str) -> usize {
    let found = names.iter().position(|candidate| *candidate == name);
    return found.unwrap_or(NO_PARENT);
}

/// Root node indices, in file order.
fn root_indices(tree: &Tree) -> Vec::<usize> {
    let mut found = Vec::<usize>::new();
    for (index, parent_index) in tree.parent.iter().enumerate() {
        if *parent_index == NO_PARENT { found.push(index); }
    }
    return found;
}

/// Node indices a path segment may match at `position`: the root indices for
/// the first segment, the children of `reached` for any later one. `reached`
/// is a valid node index whenever `position` is past the first segment, so the
/// child lookup never indexes out of range.
fn options_at(tree: &Tree, reached: usize, position: usize) -> Vec::<usize> {
    if position == 0 { return root_indices(tree); }
    return tree.children[reached].clone();
}

/// Names of the nodes at `indices`, in the same order.
fn names_at(tree: &Tree, indices: &Vec::<usize>) -> Vec::<String> {
    let mut found = Vec::<String>::new();
    for index in indices.iter() { found.push(tree.names[*index].clone()); }
    return found;
}

/// Names to offer when a failing segment follows a leaf: the reached node
/// itself and its siblings, that is the children of its parent, or the root
/// names when the reached node is itself a root. The list always contains
/// `reached`, so it is never empty.
fn fallback_names(tree: &Tree, reached: usize) -> Vec::<String> {
    let parent_index = tree.parent[reached];
    if parent_index == NO_PARENT { return names_at(tree, &root_indices(tree)); }
    return names_at(tree, &tree.children[parent_index]);
}

/// Index of a node on a cycle in the parent relation, or `None` when the
/// relations form a forest. Iterative white/grey/black colouring: every node is
/// entered at most once and left at most once, so the walk is bounded by the
/// node count and cyclic input terminates instead of chasing parents forever.
fn first_cycle_node(parent: &Vec::<usize>) -> Option::<usize> {
    const UNVISITED: usize = 0;
    const ON_PATH: usize = 1;
    const DONE: usize = 2;
    let mut state = Vec::<usize>::new();
    for _ in parent.iter() { state.push(UNVISITED); }
    for start in 0..parent.len() {
        if state[start] != UNVISITED { continue; }
        let mut path = Vec::<usize>::new();
        let mut current = start;
        let mut reached_root = false;
        while state[current] == UNVISITED {
            state[current] = ON_PATH;
            path.push(current);
            if parent[current] == NO_PARENT { reached_root = true; break; }
            current = parent[current];
        }
        if !reached_root && state[current] == ON_PATH { return Some(current); }
        for node in path.iter() { state[*node] = DONE; }
    }
    return None;
}

/// Build the category tree from `categories.csv` content.
///
/// Lines starting with `#` are comments and empty lines are ignored, per the
/// contract; carriage returns are dropped, so CRLF lines read as LF lines.
/// Every other line must be `parent;name` with a non-empty `name` and a
/// `parent` that is either empty (root) or names exactly one other row. Node
/// names must be unique across the file (see the module doc). Forward parent
/// references are allowed: parents are resolved once the whole text has been
/// read. A cycle in the `parent` relations is a typed [`BuildError.Cycle`],
/// detected by a colouring walk that terminates on cyclic input.
pub fn from_csv(text: &str) -> Result::<Tree, BuildError> {
    let mut parent_names = Vec::<String>::new();
    let mut names = Vec::<String>::new();
    for (number, line) in text.lines().enumerate() {
        if line.len() == 0 || line.starts_with("#") { continue; }
        let columns: Vec::<&str> = line.split(';').collect();
        if columns.len() != 2 || columns[1].len() == 0 { return Err(BuildError::MalformedRow(number + 1)); }
        if index_of(&names, columns[1]) != NO_PARENT { return Err(BuildError::DuplicateName(columns[1].to_string())); }
        parent_names.push(columns[0].to_string());
        names.push(columns[1].to_string());
    }
    let mut parent = Vec::<usize>::new();
    for parent_name in parent_names.iter() {
        if parent_name.len() == 0 { parent.push(NO_PARENT); continue; }
        let found = index_of(&names, parent_name);
        if found == NO_PARENT { return Err(BuildError::MissingParent(parent_name.clone())); }
        parent.push(found);
    }
    let mut children = Vec::<Vec::<usize>>::new();
    for _ in names.iter() { children.push(Vec::<usize>::new()); }
    for (position, parent_index) in parent.iter().enumerate() {
        if *parent_index != NO_PARENT { children[*parent_index].push(position); }
    }
    if let Some(index) = first_cycle_node(&parent) { return Err(BuildError::Cycle(names[index].clone())); }
    return Ok(Tree { names, parent, children });
}

/// Root category names, in file order.
pub fn roots(tree: &Tree) -> Vec::<String> {
    return names_at(tree, &root_indices(tree));
}

/// Child category names of `name`, in file order; empty for a leaf or a name
/// the tree does not contain.
pub fn children(tree: &Tree, name: &str) -> Vec::<String> {
    let position = index_of(&tree.names, name);
    if position == NO_PARENT { return Vec::<String>::new(); }
    return names_at(tree, &tree.children[position]);
}

/// Check a `parent:child` path against the tree.
///
/// The first segment must name a root, every later segment must name an
/// immediate child of the node reached so far, so a path is exactly a chain of
/// node names. Depth is uncapped: `finance:expense:transport` (three levels)
/// and deeper chains are as valid as `food:supermarket`. An unknown segment
/// yields [`PathError.UnknownCategory`] carrying that segment and the names
/// that were available at that point, so the message can suggest what the
/// caller meant.
pub fn validate(tree: &Tree, path: &str) -> Result::<String, PathError> {
    if path.len() == 0 { return Err(PathError::EmptyPath); }
    let mut reached = NO_PARENT;
    for (position, segment) in path.split(':').enumerate() {
        if segment.len() == 0 { return Err(PathError::EmptySegment); }
        let options = options_at(tree, reached, position);
        let mut found = NO_PARENT;
        for candidate in options.iter() {
            if tree.names[*candidate] == *segment { found = *candidate; break; }
        }
        if found == NO_PARENT {
            let mut candidates = names_at(tree, &options);
            if candidates.len() == 0 && position > 0 { candidates = fallback_names(tree, reached); }
            return Err(PathError::UnknownCategory(segment.to_string(), candidates));
        }
        reached = found;
    }
    return Ok(path.to_string());
}

/// True when `path` names a node of the tree.
pub fn is_valid(tree: &Tree, path: &str) -> bool {
    return validate(tree, path).is_ok();
}

/// Test helper: a comma-separated list of names as the `Vec::<String>` the
/// assertions compare against. Test-only, never part of the production code.
#[cfg(test)]
fn names_of(catalog: &str) -> Vec::<String> {
    let found: Vec::<String> = catalog.split(',').map(|name| name.to_string()).collect();
    return found;
}

/// The `categories.csv` fixture, verbatim from `testdata/categories.csv`.
#[cfg(test)]
fn fixture() -> String {
    let text = "# Phin categories -- tree of parent:child names, order is preserved.\n\
        # columns: parent;name\n\
        ;food\n\
        ;finance\n\
        ;leisure\n\
        food;supermarket\n\
        food;restaurant\n\
        finance;income\n\
        finance;expense\n\
        expense;transport\n\
        expense;utilities\n\
        leisure;books\n\
        leisure;games\n";
    return text.to_string();
}

#[test]
fn fixture_paths_validate() {
    let tree = from_csv(&fixture()).unwrap();
    assert!(is_valid(&tree, "food"));
    assert!(is_valid(&tree, "food:supermarket"));
    assert!(is_valid(&tree, "food:restaurant"));
    assert!(is_valid(&tree, "leisure:games"));
    // `supermarket` alone is not a root, so a path must start at one.
    assert!(!is_valid(&tree, "supermarket"));
    // `income` is not a child of `food`.
    assert!(!is_valid(&tree, "food:income"));
    // `transport` is a grandchild of `finance`, not an immediate child.
    assert!(!is_valid(&tree, "finance:transport"));
    assert!(!is_valid(&tree, "food:supermarket:restaurant"));
}

#[test]
fn three_level_path_is_valid() {
    let tree = from_csv(&fixture()).unwrap();
    assert!(is_valid(&tree, "finance:expense:transport"));
    assert!(is_valid(&tree, "finance:expense:utilities"));
}

#[test]
fn depth_four_is_accepted() {
    // One more level grafted onto the fixture's own `transport` node.
    let text = fixture() + "transport;commute\n";
    let tree = from_csv(&text).unwrap();
    assert!(is_valid(&tree, "finance:expense:transport:commute"));
    assert!(is_valid(&tree, "finance:expense:transport"));
    assert!(!is_valid(&tree, "finance:expense:commute"));
}

#[test]
fn unknown_category_error_lists_candidates() {
    let tree = from_csv(&fixture()).unwrap();
    let error = validate(&tree, "food:garage").unwrap_err();
    match error {
        PathError::UnknownCategory(segment, candidates) => {
            assert!(segment == "garage");
            assert!(candidates.len() > 0);
            assert!(candidates == children(&tree, "food"));
        }
        _ => { assert!(false, "expected UnknownCategory with candidates"); }
    }
    let at_root = validate(&tree, "travel").unwrap_err();
    match at_root {
        PathError::UnknownCategory(segment, candidates) => {
            assert!(segment == "travel");
            assert!(candidates == roots(&tree));
        }
        _ => { assert!(false, "expected UnknownCategory with root candidates"); }
    }
    let deep = validate(&tree, "finance:expense:holiday").unwrap_err();
    match deep {
        PathError::UnknownCategory(segment, candidates) => {
            assert!(segment == "holiday");
            assert!(candidates == children(&tree, "expense"));
        }
        _ => { assert!(false, "expected UnknownCategory with child candidates"); }
    }
}

#[test]
fn unknown_segment_below_a_leaf_still_names_candidates() {
    // `transport` is a leaf in the fixture, so its child list would be empty:
    // the payload falls back to the leaf and its siblings, never an empty list.
    let tree = from_csv(&fixture()).unwrap();
    assert!(children(&tree, "transport").len() == 0);
    let error = validate(&tree, "finance:expense:transport:garage").unwrap_err();
    match error {
        PathError::UnknownCategory(segment, candidates) => {
            assert!(segment == "garage");
            assert!(candidates.len() > 0);
            assert!(candidates == children(&tree, "expense"));
        }
        _ => { assert!(false, "expected UnknownCategory with fallback candidates"); }
    }
    // A leaf that is also a root: the fallback names the root itself.
    let only_root = from_csv(";solo\n").unwrap();
    let deep = validate(&only_root, "solo:more").unwrap_err();
    match deep {
        PathError::UnknownCategory(segment, candidates) => {
            assert!(segment == "more");
            assert!(candidates == roots(&only_root));
        }
        _ => { assert!(false, "expected UnknownCategory naming the root leaf"); }
    }
}

#[test]
fn cycle_in_parent_relations_is_a_typed_error() {
    // Two-node cycle: the walk terminates, it does not chase parents.
    let swapped = from_csv("a;b\nb;a\n").unwrap_err();
    match swapped {
        BuildError::Cycle(node) => { assert!(node == "b"); }
        _ => { assert!(false, "expected Cycle"); }
    }
    // Self-parent: a node that is its own parent.
    let self_parent = from_csv("a;a\n").unwrap_err();
    match self_parent {
        BuildError::Cycle(node) => { assert!(node == "a"); }
        _ => { assert!(false, "expected Cycle"); }
    }
    // A two-node cycle grafted onto the fixture, whose other rows stay a
    // well-formed tree: the new rows point at each other.
    let grafted = fixture() + "beta;alpha\nalpha;beta\n";
    match from_csv(&grafted).unwrap_err() {
        BuildError::Cycle(node) => { assert!(node == "alpha"); }
        _ => { assert!(false, "expected Cycle"); }
    }
}

#[test]
fn malformed_rows_report_the_line_number() {
    match from_csv("food;supermarket;extra\n") {
        Err(BuildError::MalformedRow(number)) => { assert!(number == 1); }
        _ => { assert!(false, "expected MalformedRow"); }
    }
    match from_csv("# comment\n\nnoparentcolumn\n") {
        Err(BuildError::MalformedRow(number)) => { assert!(number == 3); }
        _ => { assert!(false, "expected MalformedRow"); }
    }
    match from_csv(";\n") {
        Err(BuildError::MalformedRow(number)) => { assert!(number == 1); }
        _ => { assert!(false, "expected MalformedRow"); }
    }
}

#[test]
fn unknown_parent_reference_is_a_typed_error() {
    let error = from_csv(";food\nshop;market\n").unwrap_err();
    match error {
        BuildError::MissingParent(name) => { assert!(name == "shop"); }
        _ => { assert!(false, "expected MissingParent"); }
    }
    let broken = from_csv("b;a\n;c\n").unwrap_err();
    match broken {
        BuildError::MissingParent(name) => { assert!(name == "b"); }
        _ => { assert!(false, "expected MissingParent for b"); }
    }
    // Forward references are fine: the parent is defined further down.
    let forward = from_csv("child;parent\n;child\n").unwrap();
    assert!(roots(&forward) == names_of("child"));
    assert!(children(&forward, "child") == names_of("parent"));
    assert!(is_valid(&forward, "child:parent"));
}

#[test]
fn duplicate_node_name_is_ambiguous_and_rejected() {
    // The stricter reading the module doc commits to: a `parent` column names
    // a node, so a name defined twice makes the reference ambiguous.
    let error = from_csv(";food\n;finance\nfood;dup\nfinance;dup\n").unwrap_err();
    match error {
        BuildError::DuplicateName(name) => { assert!(name == "dup"); }
        _ => { assert!(false, "expected DuplicateName"); }
    }
    match from_csv(";food\n;food\n") {
        Err(BuildError::DuplicateName(name)) => { assert!(name == "food"); }
        _ => { assert!(false, "expected DuplicateName"); }
    }
}

#[test]
fn comments_and_blank_lines_are_ignored() {
    let text = "# leading comment\n\n;food\n\n# another\nfood;supermarket\n\n";
    let tree = from_csv(&text).unwrap();
    assert!(roots(&tree) == names_of("food"));
    assert!(children(&tree, "food") == names_of("supermarket"));
    assert!(is_valid(&tree, "food:supermarket"));
    // A comment-only file is a valid, empty tree.
    let only_comments = from_csv(&"# nothing here\n\n").unwrap();
    assert!(roots(&only_comments).len() == 0);
    assert!(!is_valid(&only_comments, "food"));
}

#[test]
fn file_order_is_preserved() {
    let tree = from_csv(&fixture()).unwrap();
    assert!(roots(&tree) == names_of("food,finance,leisure"));
    assert!(children(&tree, "food") == names_of("supermarket,restaurant"));
    assert!(children(&tree, "finance") == names_of("income,expense"));
    assert!(children(&tree, "expense") == names_of("transport,utilities"));
    assert!(children(&tree, "leisure") == names_of("books,games"));
    // Leaves have no children.
    assert!(children(&tree, "games") == Vec::<String>::new());
    // Unknown names have no children either.
    assert!(children(&tree, "nope") == Vec::<String>::new());
}

#[test]
fn empty_path_and_empty_segment_are_typed_errors() {
    let tree = from_csv(&fixture()).unwrap();
    match validate(&tree, "").unwrap_err() {
        PathError::EmptyPath => {}
        _ => { assert!(false, "expected EmptyPath"); }
    }
    match validate(&tree, "food:").unwrap_err() {
        PathError::EmptySegment => {}
        _ => { assert!(false, "expected EmptySegment"); }
    }
    match validate(&tree, "food::supermarket").unwrap_err() {
        PathError::EmptySegment => {}
        _ => { assert!(false, "expected EmptySegment"); }
    }
}
