//! Team library background requests return immediately and complete when answers arrive.
//! A server that never answers does not hold the caller up.

use std::net::TcpListener;
use std::time::{Duration, Instant};

use figcraft_engine::Session;
use serde_json::{Value, json};

use crate::test_util::{TempDir, config, publish, pump, session};

/// Start `cmd` without waiting (it must not finish at once: its answer comes over the network),
/// then pump the session until it finished; its result.
fn background(s: &mut Session, cmd: &str, params: Value) -> Value {
    let started = Instant::now();
    assert_eq!(s.start_background(cmd, params).unwrap(), None, "{cmd} waits for its answer");
    assert!(started.elapsed() < Duration::from_secs(1), "{cmd} returned at once");
    pump(&mut [&mut *s], cmd, |s| !s[0].waiting_for(cmd));
    let (done, r) = s.take_background_done().unwrap();
    assert_eq!(done, cmd);
    r.unwrap_or_else(|e| panic!("{cmd}: {e}"))
}

fn id(v: &Value) -> Value {
    v["id"].clone()
}

#[test]
fn team_library_requests_finish_in_the_background() {
    let dir = TempDir::new("background-library");
    let mut server = crate::start(&config(dir.path())).unwrap();
    let addr = server.addr();
    let mut a = session("Ada");
    let f = a.execute("frame.create", &json!({"x": 0, "y": 0, "width": 100, "height": 40, "name": "Button"})).unwrap();
    a.execute("component.create", &json!({"ids": [id(&f)]})).unwrap();
    let p = publish(&mut a, dir.path(), addr);
    // Fetching the server's current library, then publishing: two answers.
    assert_eq!(background(&mut a, "library.publishToServer", json!({}))["version"], 1);
    let mut b = session("Bo");
    let added = background(&mut b, "library.add", json!({"path": p.view}));
    assert_eq!(added["components"].as_array().map(Vec::len), Some(1));
    assert_eq!(b.doc().unwrap().libraries.enabled.len(), 1);
    let usage = background(&mut a, "library.usage", json!({}));
    assert!(usage["files"].is_array(), "{usage}");
    server.stop();
}

#[test]
fn a_server_that_never_answers_holds_nothing_up() {
    // It never accepts: connections wait in its backlog, and no answer ever comes.
    let hole = TcpListener::bind("127.0.0.1:0").unwrap();
    let server = format!("ws://{}/", hole.local_addr().unwrap());
    let mut s = session("Cy");
    let started = Instant::now();
    assert_eq!(s.start_background("collab.serverInfo", json!({"server": server})).unwrap(), None);
    assert!(started.elapsed() < Duration::from_secs(1), "the caller didn't wait");
    assert!(s.waiting_for("collab.serverInfo"));
    // Closing the window stops waiting; the answer, when the request gives up, is dropped.
    s.cancel_background("collab.serverInfo");
    assert!(!s.waiting_for("collab.serverInfo") && s.take_background_done().is_none());
    drop(hole);
}
