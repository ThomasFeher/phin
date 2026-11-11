//! phin core daemon stub.
//!
//! Reads lines from stdin, echoes back the ones it understands and ignores the
//! rest. This is a placeholder for the real core protocol
//! (see `doc/interfaces/core-protocol.md`), which replaces it in phase 2.

/// Reply for one request line, or `None` when the line is ignored.
///
/// A line starting with `echo ` is echoed back with the marker removed;
/// anything else is ignored, as is an empty line.
pub fn respond(line: String) -> Option<String> {
    const PREFIX: &str = "echo ";
    if line.starts_with(PREFIX) {
        let rest = line[PREFIX.len()..].to_string();
        return Some(rest);
    }
    return None;
}

fn main() {
    let input = std::io::stdin();
    for line in input.lines() {
        let text = match line {
            Ok(text) => { text }
            Err(_) => { break; }
        };
        let trimmed = text.trim().to_string();
        match respond(trimmed) {
            Some(reply) => { println!("{reply}"); }
            None => { }
        }
    }
}

#[test]
fn core_daemon_compiles() {}

#[test]
fn echo_line_is_repeated() {
    assert!(respond("echo ping".to_string()) == Some("ping".to_string()));
}

#[test]
fn unknown_line_is_ignored() {
    assert!(respond("ping".to_string()) == None);
    assert!(respond("".to_string()) == None);
}
