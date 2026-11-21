// ============================================================================
// Plan item 3.6, clauses (f) and (g) — the PROCESS-LEVEL HALF.
//
// The in-process tests in `core/src/technical/transport.rs` pin the
// DISCOVERY paths (spawn/reuse/stale/foreign) and the SHARED-DAEMON
// whole-command INTERLEAVING by calling the module directly. TWO clauses
// are about a daemon AS A PROCESS and CANNOT be pinned in-process:
//
//   (f) THE idle timeout EXITS THE DAEMON — the "15 minutes WITHOUT
//       COMMANDS" of `doc/architecture/core-protocol.md:16` — observed
//       here as: an interface that handshakes and NOTHING ELSE ends the
//       daemon PROCESS, and THE LOCK FILE GOES WITH IT; and
//   (g) `shutdown` ENDS THAT PROCESS AND THE LOCK IS REMOVED
//       (`core-protocol.md:199`: "exit the daemon and remove
//       `state/core.lock`").
//
// THE DAEMON HERE IS A REAL CHILD PROCESS — the executable of the spawn
// seam handed as DATA: `CARGO_BIN_EXE_core`, the integration-test form of
// "the caller's OWN argv[0] IS the executable path" (the transport
// module's decision 36: `std::process::self_exe_path` does not exist in
// this toolchain), started through `main.rs`'s OWN daemon branch with the
// flag list [`transport.spawn_arguments`] ITSELF BUILDS — so THIS file
// pins `spawn_arguments` and `main.rs`'s `daemon_settings` AGREE on
// EVERY flag: if EITHER side drifts, the child never reads its settings
// and the lock wait below FAILS the test. THE IDLE TIMEOUT TRAVELS AS
// A VALUE ON THE DAEMON'S argv (planning decision 34 — the DOCUMENTED
// config keys carry NO timeout key, inventing one is forbidden), and
// `serve_daemon` runs the SAME CODE with the ONE-SECOND value clause (f)
// can wait for; 15 MINUTES is the DEFAULT of that SAME value, NOT a
// separate path.
//
// Scratch directories live in the workspace's `test-scratch-integration/`
// (the pattern of the package's own `test-scratch/`), each named for its
// test; A DAEMON CHILD STILL ALIVE WHEN ITS TEST ENDS IS KILLED — its
// clean exit was ALREADY ASSERTED before cleanup; the kill only protects
// the OTHER tests from an ORPHAN.
// ============================================================================


use std::io::Read;
use std::io::Write;
#[cfg(test)]
fn scratch(name: String) -> std::path::PathBuf {
    let dir = std::path::PathBuf::from("test-scratch-integration").join(name).to_path_buf();
    match std::fs::remove_dir_all(&dir) {
        Err(_) => { }
        Ok(_) => { }
    }
    match std::fs::create_dir_all(&dir) {
        Err(_) => { }
        Ok(_) => { }
    }
    return dir;
}

#[cfg(test)]
fn cleanup(dir: &std::path::PathBuf) {
    match std::fs::remove_dir_all(dir) {
        Err(_) => { }
        Ok(_) => { }
    }
    // The EMPTY parent goes too, best effort (the convention of
    // `core/src/technical/view.rs`: a scratch dir is CREATED AND REMOVED
    // by its test, so after a test run `find . -name 'test-scratch*'`
    // reads EMPTY); a sibling test still RUNNING keeps it non-empty and
    // this removal is simply REFUSED.
    match std::fs::remove_dir(std::path::PathBuf::from("test-scratch-integration")) {
        Err(_) => { }
        Ok(_) => { }
    }
}

#[cfg(test)]
fn free_port(base: i64) -> i64 {
    // A port NO listener holds: the FIRST port from a per-test BASE a fresh
    // BIND SUCCEEDS ON (the bind IS the proof; the probe listener dies at
    // the end of the scope releasing it). This toolchain has NO accessor
    // for the port an `:0` bind chose, and every test's base is DISTINCT
    // and NONE touches the documented default 47623.
    let mut candidate = base;
    while candidate < base + 50 {
        let short: u16 = candidate.try_into().unwrap_or(0);
        match std::net::TcpListener::bind(("127.0.0.1", short)) {
            Err(_) => { }
            Ok(_) => { return candidate; }
        }
        candidate += 1;
    }
    return base;
}

#[cfg(test)]
fn stop_daemon(child: &mut std::process::Child) {
    // KILL A DAEMON CHILD STILL RUNNING (failure paths only — the success
    // paths have ALREADY waited the clean exit out).
    match child.kill() {
        Err(_) => { }
        Ok(_) => { }
    }
}

#[cfg(test)]
fn wait_for_daemon_exit(_child: &std::process::Child, host: String, port: i64) -> bool {
    // A BOUNDED wait for the daemon PROCESS to exit on its own — the row's
    // rule that NO test may HANG: every child wait carries a wall-clock limit
    // so a mutant daemon that never self-exits FAILS instead of hanging the
    // suite. This toolchain has NO `Child.wait_timeout` and NO stdlib channel
    // (probes: `std::thread::Channel` and `std::sync::BlockingQueue` -> E0433
    // "could not find"), and `TcpStream::connect_timeout` is UNREACHABLE
    // (E0061 wants a `&SocketAddr`, whose value constructors E0423/`SocketAddrV4.ip`
    // E0308 are not expressible in this std) — toolchain facts of this row.
    // So the bound is a POLL OF LIVENESS BY AN ATTEMPTED OPERATION: while
    // the daemon PROCESS lives it holds the ONLY listener socket on its
    // configured port, so a fresh `TcpListener::bind` on that port is
    // REFUSED; the moment the PROCESS EXITS the bind SUCCEEDS (the probe
    // listener is dropped at the end of the scope, releasing the port again).
    // That is the SOCKET the architecture names as the daemon's identity
    // (`architecture.md:247`: the port and its handshake, NEVER a process
    // id, is the liveness test). Unlike a connect-probe, a failed BIND
    // attempt CANNOT reset the daemon's idle clock — the daemon never even
    // sees the attempt. Once a BIND SUCCEEDS the process has ALREADY EXITED,
    // so the caller's `child.wait()` only REAPS the zombie and returns
    // instantly — never unbounded. POLL PAUSE 100 ms; budget 20 SECONDS.
    let mut tries: i64 = 0;
    while tries < 200 {
        let short: u16 = port.try_into().unwrap_or(0);
        match std::net::TcpListener::bind((host.to_string(), short)) {
            Err(_) => {
                // The port is STILL held: the daemon process is alive.
                // Keep waiting, bounded.
                std::thread::sleep(std::time::Duration::from_millis(100));
                tries += 1;
            }
            Ok(_) => {
                // The port is FREE: the daemon PROCESS that held its listener
                // has EXITED by itself.
                return true;
            }
        }
    }
    return false;
}

#[cfg(test)]
fn executable_path() -> String {
    // The executable of the spawn seam AS DATA — cargo's
    // `CARGO_BIN_EXE_core` (the integration-test form of "`main.rs`'s
    // OWN argv[0]": the transport hands the executable of the daemon
    // child as DATA because `std::process::self_exe_path` does not
    // exist). A MISSING variable is an EMPTY string HERE, and EVERY
    // caller asserts on the child it PRODUCES, so the absence FAILS
    // LOUDLY.
    match std::env::var("CARGO_BIN_EXE_core") {
        Err(_) => { return String::new(); }
        Ok(held) => { return held.to_string(); }
    }
}

#[cfg(test)]
struct DaemonChild {
    pub child: Option::<std::process::Child>,
    pub host: String,
    pub port: i64,
    pub nonce: String,
}

#[cfg(test)]
fn start_daemon(dir: &std::path::PathBuf, host: String, port: i64, idle_seconds: i64) -> DaemonChild {
    // The `phin.toml` of the scratch dir points at the HOST AND FREE port
    // GIVEN: this pins, ACROSS THE PROCESS BOUNDARY, that a CONFIGURED
    // NON-DEFAULT HOST is really APPLIED — the CHILD binds and handshakes
    // the HOST the configuration names, and with the mutant "discard the
    // parsed host, ALWAYS DEFAULT_ADDRESS" the CHILD would listen where
    // the interface does NOT connect and the lock wait below would never
    // produce a NONCE (the review of plan item 3.6b recorded that the
    // `127.0.0.1`-only tests could not SEE such a mutant at all). It
    // ALSO pins the child's `--data-dir=` flag really steers the child:
    // the lock file appears UNDER this dir, which the DEFAULT data dir
    // (the current directory of `data-files.md:152`) NEVER is.
    match std::fs::write(&dir.join("phin.toml"), format!("core.address = \"{at_host}:{at}\"\n", at_host = host.to_string(), at = port.to_string())) {
        Err(_) => { }
        Ok(_) => { }
    }
    let endpoint = core::technical::transport_cfg::Endpoint { host: host.to_string(), port: port };
    let args = core::technical::transport::spawn_arguments(&endpoint, dir, idle_seconds);
    match args {
        None => { return DaemonChild { child: None, host: host.to_string(), port: port, nonce: String::new() }; }
        Some(held_args) => {
            match std::process::Command::new(&executable_path()).args(&held_args).spawn() {
                Err(_) => { return DaemonChild { child: None, host: host.to_string(), port: port, nonce: String::new() }; }
                Ok(child) => {
                    // WAIT FOR THE LOCK FILE — the child binds THEN writes
                    // the lock THEN serves (decision 35's readiness signal;
                    // `architecture.md:248`: interfaces find the daemon
                    // THROUGH THE LOCK FILE), polled through the transport's
                    // OWN `read_lock`. A child that NEVER came up hands
                    // back an EMPTY NONCE and EVERY caller asserts on it.
                    let mut tries: i64 = 0;
                    while tries < 160 {
                        match core::technical::transport_cfg::read_lock(dir) {
                            core::technical::transport_cfg::LockReading::Read(record) => {
                                return DaemonChild { child: Some(child), host: host.to_string(), port: port, nonce: record.nonce.to_string() };
                            }
                            _ => { }
                        }
                        std::thread::sleep(std::time::Duration::from_millis(50));
                        tries += 1;
                    }
                    return DaemonChild { child: Some(child), host: host.to_string(), port: port, nonce: String::new() };
                }
            }
        }
    }
}

#[cfg(test)]
fn shutdown_frame() -> String {
    // The DOCUMENTED `shutdown` line (`core-protocol.md:199`,
    // `proto::command::Command::Shutdown` of the command table) encoded
    // by plan 3.1's codec. NOTE the arm the interpreter stops on:
    // `interpret.rs:392` maps the `shutdown` COMMAND to
    // `Stop::ShutdownRequested` (the EVENT spelling of the same rule,
    // `interpret.rs:320`, is pinned in the in-process tests of the
    // transport module; BOTH land on `SessionEnd::ShutDown` there, and
    // the CHILD's exit path is the SAME `serve_daemon` EITHER WAY).
    match proto::codec::encode_frame(&proto::codec::Message::Inbound(proto::command::Command::Shutdown)) {
        Err(_) => { return String::new(); }
        Ok(frame) => { return frame.to_string(); }
    }
}

#[test]
fn a_daemon_child_handed_only_a_handshake_exits_on_its_idle_timeout_and_removes_its_lock() {
    // Clause (f). THE interface of THIS test IS THE HANDSHAKE ITSELF —
    // the version request of [`transport.open_session`] answered and
    // CHECKED by the CHILD — and NOTHING ELSE was EVER SENT ("WITHOUT
    // COMMANDS"). WHEN the connection DIES (the session's sockets closing
    // at the end of its scope = the interface VANISHING) THE DAEMON'S
    // ACCEPT LOOP MEASURES ITS IDLE TIMEOUT AND THE PROCESS EXITS:
    // `wait()` answers, the exit code is main's CLEAN 0 (`daemon_status`:
    // `IdleExit` + lock REMOVED → 0) AND THE LOCK FILE IS GONE — both
    // halves of `architecture.md:248`'s "removed on exit" seen from
    // OUTSIDE the process. THE ONE-SECOND `--idle-timeout=` is the SAME
    // VALUE channel plan 3.5 will drive (decision 34), 15 minutes being
    // the DEFAULT OF that value rather than a DIFFERENT code path.
    let dir = scratch("a_daemon_child_handed_only_a_handshake_exits_on_its_idle_timeout_and_removes_its_lock".to_string());
    // THE CONFIGURED HOST IS `localhost`, NOT the DEFAULT: seen from
    // OUTSIDE the child, a dropped configured host would put the CHILD
    // at the DEFAULT address while the interface still connects at
    // `localhost` (this toolchain binds the two names as distinct loopback
    // addresses — a probe showed a `localhost` bind is NOT seen by a
    // `127.0.0.1` connect), so the handshake below could not answer at
    // all: the NON-DEFAULT HOST is pinned end-to-end, not just in the
    // configuration unit test.
    let port = free_port(41101);
    let daemon = start_daemon(&dir, "localhost".to_string(), port, 1);
    match daemon.child {
        None => {
            cleanup(&dir);
            assert!(false);
        }
        Some(mut child) => {
            if daemon.nonce.len() != 16 {
                stop_daemon(&mut child);
                cleanup(&dir);
                assert!(false);
            }
            {
                // THE interface: ONE open session and NOTHING ELSE, EVER.
                let endpoint = core::technical::transport_cfg::Endpoint { host: daemon.host.to_string(), port: port };
                let held = core::technical::transport::open_session(&endpoint, daemon.nonce.to_string(), 5, 1);
                match held.1 {
                    core::technical::transport::Attempt::Handshook(_, _) => { }
                    _ => {
                        stop_daemon(&mut child);
                        cleanup(&dir);
                        assert!(false);
                    }
                }
                // THE session id of the handshake is the daemon's OWN,
                // derived from the lock NONCE by proto's OWN function
                // (decision 33's identity rule seen ACROSS A PROCESS
                // BOUNDARY):
                match held.0 {
                    Some(connection) => {
                        // THE session id of the handshake is the daemon's
                        // OWN, derived from the lock NONCE by proto's OWN
                        // function (decision 33's identity rule seen
                        // ACROSS A PROCESS BOUNDARY), and the address the
                        // connection names is THE PORT THE SCRATCH
                        // CONFIGURATION POINTED AT — the child really
                        // serves THE CONFIGURED ENDPOINT, not the default:
                        if connection.session != proto::handshake::session_id_of(&daemon.nonce.to_string()) {
                            stop_daemon(&mut child);
                            cleanup(&dir);
                            assert!(false);
                        }
                        if connection.address != core::technical::transport_cfg::endpoint_text(daemon.host.to_string(), daemon.port) {
                            stop_daemon(&mut child);
                            cleanup(&dir);
                            assert!(false);
                        }
                    }
                    None => {
                        stop_daemon(&mut child);
                        cleanup(&dir);
                        assert!(false);
                    }
                }
                // THE SCOPE ENDING CLOSES THE CONNECTION: the daemon's
                // read of this session's stream ENDS AT EOF, and its
                // accept loop then MEASURES the idle timeout and EXITS.
            }
            // THE PROCESS ITSELF, BOUNDED (the row's NO-test-may-HANG rule):
            // `wait_for_daemon_exit` POLLS the daemon's OWN listener socket
            // until the connect is REFUSED — which can only happen once the
            // DAEMON PROCESS EXITED BY ITSELF — within a 20 SECOND wall-clock
            // budget; a daemon that would NOT exit makes the POLL exhaust and
            // the test FAIL (via `stop_daemon` + `assert`), NEVER HANG. Only
            // THEN is `child.wait()` called, and there it only REAPS an
            // already-EXITED process, so it cannot block. A kill-first would
            // HIDE the non-exiting bug; here the non-exiting daemon is
            // DETECTED, killed for the OTHER tests' sake, and reported.
            if !wait_for_daemon_exit(&child, daemon.host.to_string(), port) {
                stop_daemon(&mut child);
                cleanup(&dir);
                assert!(false);
            }
            match child.wait() {
                Err(_) => {
                    cleanup(&dir);
                    assert!(false);
                }
                Ok(status) => {
                    match status.code() {
                        None => {
                            cleanup(&dir);
                            assert!(false);
                        }
                        Some(held_code) => {
                            if held_code != 0 {
                                cleanup(&dir);
                                assert!(false);
                            }
                        }
                    }
                }
            }
            // THE LOCK FILE IS GONE — `IdleExit` REMOVED IT BEFORE
            // exiting, read back through the transport's OWN `read_lock`.
            // AND the lock file's OWN address text is the CONFIGURED
            // `localhost:PORT` the interface used... it is GONE, so the
            // address pin here is the CONNECTION'S above; the daemon
            // BINDING THE CONFIGURED HOST is proven by the handshake
            // ANSWERING AT `localhost` at all.
            match core::technical::transport_cfg::read_lock(&dir) {
                core::technical::transport_cfg::LockReading::Absent => { }
                _ => {
                    println!("lock still present when the daemon exited");
                    cleanup(&dir);
                    assert!(false);
                }
            }
            cleanup(&dir);
        }
    }
}

#[test]
fn a_real_interface_sending_shutdown_ends_the_daemon_child_and_the_lock_is_gone() {
    // Clause (g) — WHAT could NOT be pinned in-process (the in-process
    // shutdown test already pins `shutdown` → `DaemonExit::ShutdownExit(
    // Removal::Removed)` and the file GONE): THE DAEMON PROCESS ITSELF
    // EXITING ON a shutdown sent through A REAL interface. The interface
    // here IS the connection [`transport.open_session`] HANDS OUT against
    // the live CHILD writing the DOCUMENTED shutdown line on its OWN
    // sink, THEN DYING. `interpret::run` STOPS on the command
    // (`interpret.rs:392`), the transport answers `SessionEnd::ShutDown`,
    // `serve_daemon` EXITS with `ShutdownExit` REMOVING THE LOCK, and
    // `main.rs`'s `daemon_status` ENDS the PROCESS CLEAN.
    // NOTE THE ONE-MINUTE IDLE HERE — if the shutdown did NOTHING, the
    // idle could NOT rescue the test EITHER within the row's wall-clock
    // budget: the wait below is BOUNDED at 20 SECONDS, so a daemon IGNORING
    // shutdown FAILS the test long before its 60-second idle could ever
    // clean it away (a kill-first would HIDE that bug; the bounded liveness
    // poll DETECTS it and reports).
    let dir = scratch("a_real_interface_sending_shutdown_ends_the_daemon_child_and_the_lock_is_gone".to_string());
    let port = free_port(41201);
    let daemon = start_daemon(&dir, "127.0.0.1".to_string(), port, 60);
    match daemon.child {
        None => {
            cleanup(&dir);
            assert!(false);
        }
        Some(mut child) => {
            if daemon.nonce.len() != 16 {
                stop_daemon(&mut child);
                cleanup(&dir);
                assert!(false);
            }
            let endpoint = core::technical::transport_cfg::Endpoint { host: "127.0.0.1".to_string(), port: port };
            let held = core::technical::transport::open_session(&endpoint, daemon.nonce.to_string(), 5, 60);
            match held.0 {
                None => {
                    stop_daemon(&mut child);
                    cleanup(&dir);
                    assert!(false);
                }
                Some(mut connection) => {
                    if connection.session != proto::handshake::session_id_of(&daemon.nonce.to_string()) {
                        stop_daemon(&mut child);
                        cleanup(&dir);
                        assert!(false);
                    }
                    // THE interface writes the shutdown line THROUGH THE
                    // SAME SOCKET THE CONNECTION'S SINK WRAPS — the
                    // `EventSink` trait method would need THIS FILE to
                    // IMPORT THAT TRAIT, and writing the ONE LINE
                    // DIRECTLY IS the interface's OWN OBSERVABLE (the
                    // flush mirrors plan 3.3's sink obligation; the
                    // latch below latches the FAILURE the same way).
                    match connection.sink.writer.write_all(shutdown_frame().as_bytes()) {
                        Err(_) => { }
                        Ok(_) => {
                            match connection.sink.writer.flush() {
                                Err(_) => { }
                                Ok(_) => { }
                            }
                        }
                    }
                    if connection.sink.broken {
                        stop_daemon(&mut child);
                        cleanup(&dir);
                        assert!(false);
                    }
                    // THE connection THEN DIES (its scope ENDS): the
                    // shutdown LINE was WRITTEN BEFORE the close, so the
                    // daemon reads IT from its stream BEFORE ANY EOF
                    // could race it away.
                }
            }
            // THE PROCESS ITSELF, BOUNDED: same wall-clock-poll route as the
            // idle test — the daemon's listener socket stops ACCEPTING
            // connections only when the PROCESS IS GONE — then `child.wait()`
            // REAPS the already-exited child, so nothing here can hang.
            if !wait_for_daemon_exit(&child, daemon.host.to_string(), port) {
                stop_daemon(&mut child);
                cleanup(&dir);
                assert!(false);
            }
            match child.wait() {
                Err(_) => {
                    cleanup(&dir);
                    assert!(false);
                }
                Ok(status) => {
                    match status.code() {
                        None => {
                            cleanup(&dir);
                            assert!(false);
                        }
                        Some(held_code) => {
                            if held_code != 0 {
                                cleanup(&dir);
                                assert!(false);
                            }
                        }
                    }
                }
            }
            // THE LOCK IS GONE: plan 3.5's rule "the daemon exits and
            // removes the lock" seen from THE OUTSIDE.
            match core::technical::transport_cfg::read_lock(&dir) {
                core::technical::transport_cfg::LockReading::Absent => { }
                _ => {
                    println!("lock still present when the daemon exited");
                    cleanup(&dir);
                    assert!(false);
                }
            }
            cleanup(&dir);
        }
    }
}


// ---------------------------------------------------------------------------
// Clause (e), PROCESS-LEVEL: whole-command-by-whole-command interleaving
// across TWO LIVE INTERFACES OF ONE SPAWNED CHILD — and the CONNECION the
// scheduler serializes
// ---------------------------------------------------------------------------

#[cfg(test)]
struct LiveSessionSpec {
    pub host: String,
    pub port: i64,
    pub nonce: String,
    pub answered_path: String,
    pub lines_path: String
}

#[cfg(test)]
fn timeout_value(seconds: i64) -> Option::<std::time::Duration> {
    // THE `Option` WRAPPER `set_read_timeout` TAKES in THIS toolchain
    // (the transport's OWN `set_read_deadline` wraps the same way):
    // a DEADLINE is an OPTION, the ABSENCE of one is "wait forever".
    match seconds.try_into() {
        Err(_) => { return Some(std::time::Duration::from_secs(0)); }
        Ok(unsigned) => { return Some(std::time::Duration::from_secs(unsigned)); }
    }
}

#[cfg(test)]
fn command_frame(command: proto::command::Command) -> String {
    // THE FRAME of ONE table command THROUGH PLAN 3.1's OWN codec —
    // `shutdown_frame`'s pattern generalized, so the CHILD's error lines
    // carry the command names the codec itself puts in `cmd=`.
    match proto::codec::encode_frame(&proto::codec::Message::Inbound(command)) {
        Err(_) => { return String::new(); }
        Ok(frame) => { return frame.to_string(); }
    }
}

#[cfg(test)]
fn live_session_work(spec: LiveSessionSpec) {
    // THE SECOND LIVE INTERFACE, RUNNING ON ITS OWN THREAD AGAINST THE
    // CHILD THE MAIN TEST THREAD SPAWNED. `std::thread::spawn` takes a
    // ZERO-ARGUMENT closure and this toolchain's free-variable captures
    // E0373 (the struct-by-value pattern `transport.rs`'s fake-peer seam
    // already uses), so the endpoint, NONCE and output paths arrive as
    // the BYTES of `LiveSessionSpec`. What the thread does: CONNECT +
    // HANDSHAKE — which the CHILD's scheduler answers ONLY WHEN IT
    // FINISHED THE FIRST READER'S SESSION, the doc's one-connection-at-
    // a-time boundary (`architecture.md:389`: a second live connection
    // WAITS in the backlog) — then write `answered` as FAST as the
    // handshake is up so the MAIN THREAD SEES the MOMENT this session
    // STARTED, then the `report` command and the `shutdown` line through
    // the CONNECTION's OWN SINK SOCKET (`core-protocol.md:4`), then EVERY
    // LINE THIS READER RECEIVED to `lines_path` for the MAIN THREAD.
    let endpoint = core::technical::transport_cfg::Endpoint { host: spec.host.to_string(), port: spec.port };
    let held = core::technical::transport::open_session(&endpoint, spec.nonce.to_string(), 30, 60);
    match held.0 {
        None => {
            // THE HANDSHAKE NEVER CAME UP (a MUTANT that never reaches
            // this backlog connection runs into the 30 s deadline HERE):
            // record REFUSED, and the MAIN THREAD's bounded wait reports
            // the failure — NOT a hang.
            match std::fs::write(&std::path::PathBuf::from(spec.answered_path.to_string()), "refused") {
                Err(_) => { }
                Ok(_) => { }
            }
            return;
        }
        Some(mut connection) => {
            match std::fs::write(&std::path::PathBuf::from(spec.answered_path.to_string()), "answered") {
                Err(_) => { }
                Ok(_) => { }
            }
            let report = proto::command::Command::Report("markdown".to_string(), None);
            for frame_text in [command_frame(report), shutdown_frame()] {
                match connection.sink.writer.write_all(frame_text.as_bytes()) {
                    Err(_) => { }
                    Ok(_) => { }
                }
            }
            match connection.sink.writer.flush() {
                Err(_) => { }
                Ok(_) => { }
            }
            // EVERY LINE RECEIVED, `shutdown` included. The CHILD EXITS
            // AT the shutdown line — `shutdown` NEVER ANSWERS
            // (`core-protocol.md:22`) — so this socket then DIES, and
            // `std::io`'s `WouldBlock` ends the loop:
            match connection.text.reader.set_read_timeout(timeout_value(5)) {
                Err(_) => { }
                Ok(_) => { }
            }
            let mut lines = String::new();
            loop {
                let mut buffer = vec![0u8];
                match connection.text.reader.read(&mut buffer) {
                    Err(_) => { break; }
                    Ok(held) => {
                        if held == 0 { break; }
                        match String::from_utf8(buffer) {
                            Err(_) => { }
                            Ok(decoded) => { lines += &decoded; }
                        }
                    }
                }
            }
            match std::fs::write(&std::path::PathBuf::from(spec.lines_path.to_string()), lines) {
                Err(_) => { }
                Ok(_) => { }
            }
        }
    }
}

#[cfg(test)]
fn drain_lines(mut socket: std::net::TcpStream) -> String {
    // EVERY LINE THE FIRST READER RECEIVED, read AFTER the CHILD IS
    // CONFIRMED GONE (port free), so EOF ENDS the loop IMMEDIATELY: the
    // CHILD CLOSED THIS SOCKET AT THE END OF ITS SESSION — `shutdown`
    // NEVER ANSWERS, the process is EXITED — and everything this
    // reader's session ever received sits in the socket's buffer.
    let mut lines = String::new();
    match socket.set_read_timeout(timeout_value(5)) {
        Err(_) => { }
        Ok(_) => { }
    }
    loop {
        let mut buffer = vec![0u8];
        match socket.read(&mut buffer) {
            Err(_) => { break; }
            Ok(held) => {
                if held == 0 { break; }
                match String::from_utf8(buffer) {
                    Err(_) => { }
                    Ok(decoded) => { lines += &decoded; }
                }
            }
        }
    }
    return lines;
}

#[cfg(test)]
fn shape_of_lines(text: String) -> String {
    // THE WHOLE-COMMAND INTERLEAVING OBSERVABLE IN THE SAME SHAPE THE
    // IN-PROCESS scheduler test asserts: for EVERY decodable line of
    // one reader's stream, the event KIND plus — for the CHILD's error
    // lines — the `cmd` NAME the line CARRIES (`interpret`'s
    // [`WiredNowhere` fills it from plan 3.2's table), entries joined by
    // `|`. The CHILD NEVER emits MULTI-LINE blocks — every table command
    // here answers EXACTLY ONE line — so a reader's block list is
    // complete IFF a command of the OTHER reader's stream sits INSIDE
    // it, and BOTH tests check THE SAME FAMILY: the FIRST entry of each
    // reader is a `hello` (the handshake), EVERY command of that reader
    // appears as ONE CONTIGUOUS GROUP in send order, and NOTHING of
    // the other reader is inside a group: the interleaving unit is the
    // WHOLE command, never the line.
    let mut parts = Vec::<String>::new();
    for line in text.split("\n") {
        if line.len() == 0 { continue; }
        match proto::codec::decode_line(&line) {
            Err(_) => { }
            Ok(held) => {
                match held {
                    proto::codec::Message::Outbound(event) => {
                        let mut entry = proto::event::event_kind(&event);
                        match event {
                            proto::event::Event::Error(cmd, _, _) => {
                                match cmd {
                                    Some(name) => { entry = format!("{at_cmd}:{at_name}", at_cmd = entry, at_name = name.to_string()); }
                                    None => { }
                                }
                            }
                            _ => { }
                        }
                        parts.push(entry);
                    }
                    _ => { }
                }
            }
        }
    }
    return parts.join("|");
}

#[test]
fn two_live_interfaces_on_one_spawned_daemon_interleave_whole_command_by_whole_command_never_line_by_line() {
    // CLAUSE (e) AT PROCESS GRANULARITY. The IN-PROCESS scheduler test
    // of the same clause drives THE SAME THREE FUNCTIONS
    // ([`transport.serve_daemon`] -> [`transport.serve_one_connection`]
    // -> `interpret::run_session`) IN ONE PROCESS with TWO REAL SOCKET
    // CONNECTIONS AND AN INJECTED MULTI-LINE `Work` — the SAME CODE PATH
    // the SPAWNED CHILD RUNS INSIDE ITSELF. This test drives those
    // THREE FUNCTIONS THROUGH ONE SPAWNED CHILD with TWO REAL SOCKET
    // CONNECTIONS AND THE CHILD'S OWN PRODUCTION WORK (`interpret`'s
    // wired table: EVERY command answers EXACTLY ONE `error` line naming
    // ITS `cmd`), and asserts THE SAME OBSERVABLE SHAPE at BOTH
    // granularities — one block per command, contiguous, in send order,
    // NEVER A LINE INSIDE THE WRONG COMMAND'S BLOCK — PLUS THE
    // CONNECTION-LEVEL serialization ONLY A LIVE CHILD EXPOSES: READER
    // B'S handshake is answered ONLY AFTER READER A'S SESSION ENDS
    // (`architecture.md:389`: the backlog waits). Frame of the test:
    // the CHILD's daemon runs with a 60 s idle timeout — NEVER the
    // trigger here — and `shutdown` ENDS IT, whose exit is CONFIRMED by
    // THE FREE-PORT OBSERVATION (`wait_for_daemon_exit`), and the lock
    // removal after the shutdown exit pins `data-files.md:147` AND
    // `core-protocol.md:22` from the CHILD's side of the seam.
    let dir = scratch("two_live_interfaces_on_one_spawned_daemon_interleave_whole_command_by_whole_command_never_line_by_line".to_string());
    let port = free_port(41301);
    let daemon = start_daemon(&dir, "127.0.0.1".to_string(), port, 60);
    match daemon.child {
        None => {
            cleanup(&dir);
            assert!(false);
        }
        Some(mut child) => {
    if daemon.nonce.len() != 16 {
        stop_daemon(&mut child);
        cleanup(&dir);
        assert!(false);
    }
    // READER A: A SESSION THAT STAYS OPEN THE WHOLE TEST — the handshake
    // completes (the daemon is here to accept it) and A's TWO commands
    // (`import`, `list-pending`) go THROUGH the connection's OWN SINK
    // SOCKET; A NEVER sends `shutdown` and A's WRITE END CLOSES only at
    // THE OBSERVED MOMENT.
    let endpoint_a = core::technical::transport_cfg::Endpoint { host: daemon.host.to_string(), port: port };
    let held_a = core::technical::transport::open_session(&endpoint_a, daemon.nonce.to_string(), 5, 60);
    match held_a.1 {
        core::technical::transport::Attempt::Handshook(_, _) => { }
        _ => {
            stop_daemon(&mut child);
            cleanup(&dir);
            assert!(false);
        }
    }
    match held_a.0 {
        None => {
            stop_daemon(&mut child);
            cleanup(&dir);
            assert!(false);
        }
        Some(mut connection_a) => {
            // A's COMMAND FRAMES, WRITTEN THROUGH THE CONNECTION'S OWN
            // SINK SOCKET — `core-protocol.md:4`: commands go to the
            // sink end of the daemon connection, exactly as test (g)'s
            // shutdown frame does:
            let import = proto::command::Command::Import("the-garage".to_string());
            let pending = proto::command::Command::ListPending;
            for frame_text in [command_frame(import), command_frame(pending)] {
                match connection_a.sink.writer.write_all(frame_text.as_bytes()) {
                    Err(_) => { }
                    Ok(_) => { }
                }
            }
            match connection_a.sink.writer.flush() {
                Err(_) => { }
                Ok(_) => { }
            }
            let answered_path = dir.join("b_answered.txt");
            let lines_path = dir.join("b_lines.txt");
            let spec = LiveSessionSpec {
                host: daemon.host.to_string(),
                port: port,
                nonce: daemon.nonce.to_string(),
                answered_path: answered_path.to_string_lossy().to_string(),
                lines_path: lines_path.to_string_lossy().to_string()
            };
            // READER B ON ITS OWN THREAD — the SECOND LIVE INTERFACE
            // of THE SAME child:
            let session_thread = std::thread::spawn(|| -> () { live_session_work(spec); });
            // `join` CONSUMES ITS HANDLE in THIS toolchain, so the
            // FAILURE branches below NEVER touch it — the HANDLE
            // DROPS AT THE assert's UNWIND, DETACHING the thread,
            // WHICH IS BOUNDED ON ITS OWN (5 s READ DEADLINE, and
            // the branch has already STOPPED THE DAEMON so its
            // socket DIES within SECONDS); ONLY the SUCCESS path
            // below joins, AFTER the CHILD'S exit is CONFIRMED, so
            // the lines file is COMPLETE when read.
            // THE SERIALIZATION OBSERVABLE FROM THE MAIN THREAD: B
            // CONNECTED AND SENT its version request LONG BEFORE NOW,
            // AND THE CHILD'S SCHEDULER IS STILL INSIDE A'S SESSION —
            // so THE answered FILE CANNOT EXIST YET. THE SLEEP IS NOT
            // THE PROOF (A MUTANT that served B CONCURRENTLY WOULD FAIL
            // HERE INSTEAD OF PASSING LATER):
            std::thread::sleep(std::time::Duration::from_millis(1000));
            match std::fs::exists(&answered_path) {
                Err(_) => { }
                Ok(seen) => {
                    if seen {
                        stop_daemon(&mut child);
                        
                        cleanup(&dir);
                        assert!(false);
                    }
                }
            }
            // NOW A'S SESSION ENDS — A's write end closes (`shutdown`
            // would KILL THE CHILD and prove NOTHING about the session
            // boundary; EOF is A's closing the connection, and the
            // connection close IS a session end in `data-files.md:160`
            // 's frame model):
            match connection_a.text.reader.shutdown(std::net::Shutdown::Write) {
                Err(_) => { }
                Ok(_) => { }
            }
            // THE MOMENT B'S SESSION STARTED, BOUNDED: poll the
            // answered file. The CHILD NEEDS NO KILL on this path —
            // B's OWN shutdown line ENDS IT — and the wait below
            // bounds EVERY step:
            let mut tries = 0;
            let mut answered = false;
            let mut refused = false;
            while tries < 200 {
                match std::fs::read_to_string(&answered_path) {
                    Err(_) => { }
                    Ok(seen) => {
                        if seen == "answered".to_string() {
                            answered = true;
                            break;
                        }
                        if seen == "refused".to_string() {
                            refused = true;
                            break;
                        }
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
                tries += 1;
            }
            if refused {
                println!("reader B's handshake was REFUSED, not ANSWERED");
            }
            if !answered {
                stop_daemon(&mut child);
                
                cleanup(&dir);
                assert!(false);
            }
            // B'S SESSION NOW RUNS TO THE CHILD'S EXIT THROUGH B'S OWN
            // shutdown LINE; THE CHILD'S EXIT IS CONFIRMED, NOT ASSUMED:
            if !wait_for_daemon_exit(&child, "127.0.0.1".to_string(), port) {
                stop_daemon(&mut child);
                
                cleanup(&dir);
                assert!(false);
            }
            match child.wait() {
                Err(_) => {
                    
                    cleanup(&dir);
                    assert!(false);
                }
                Ok(status) => {
                    match status.code() {
                        Some(held_code) => {
                            if held_code != 0 {
                                
                                cleanup(&dir);
                                assert!(false);
                            }
                        }
                        None => {
                            
                            cleanup(&dir);
                            assert!(false);
                        }
                    }
                }
            }
            match session_thread.join() {
                Err(_) => { cleanup(&dir); assert!(false); }
                Ok(_) => { }
            }
            // THE CHILD REMOVED ITS OWN LOCK ON THE SHUTDOWN EXIT —
            // `data-files.md:147` AND `core-protocol.md:22` SEEN FROM
            // THE CHILD'S SIDE OF THE SEAM:
            match core::technical::transport_cfg::read_lock(&dir) {
                core::technical::transport_cfg::LockReading::Absent => { }
                _ => {
                    cleanup(&dir);
                    assert!(false);
                }
            }
            // READER A'S STREAM, DRAINED AFTER THE CHILD DIED (EOF
            // ENDS the helper): [`open_session`] ALREADY CONSUMED A's
            // handshake ANSWER — the HELLO line is gone from this socket
            // by the time the session starts — so A's stream is EXACTLY
            // its TWO command blocks, `import` THEN `list-pending`, one
            // CONTIGUOUS block each in send order, and NONE of READER
            // B's lines (its stream is ONE `error:report`) sits INSIDE
            // one of A's blocks: the exact shape
            // `whole_command_interleaving_never_line_interleaving` pins
            // for MULTI-LINE blocks, at the CHILD's one-line granularity.
            let a_shape = shape_of_lines(drain_lines(connection_a.text.reader));
            if a_shape != "error:import|error:list-pending".to_string() {
                cleanup(&dir);
                assert!(false);
            }
            // READER B'S STREAM, SAME SHAPE RULE: ITS HANDSHAKE ANSWER
            // was CONSUMED BY ITS OWN `open_session`, so what the
            // thread recorded is ITS ONE command block and NOTHING of
            // A's stream inside it:
            match std::fs::read_to_string(&lines_path) {
                Err(_) => {
                    cleanup(&dir);
                    assert!(false);
                }
                Ok(seen) => {
                    if shape_of_lines(seen) != "error:report".to_string() {
                        cleanup(&dir);
                        assert!(false);
                    }
                }
            }
            cleanup(&dir);
        }
    }
        }
    }
}