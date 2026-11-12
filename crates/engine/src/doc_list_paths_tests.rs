//! `doc.list` tells the user's own scripts where the open files are, but AI agents and plugins
//! get their names only (#1117).

use serde_json::json;

use crate::Session;
use crate::components::tests::ex;

const SECRET: &str = "/home/someone/private/clients/acme.figcraft";

fn two_files() -> Session {
    let mut s = Session::new();
    s.docs[0].path = Some(SECRET.into());
    ex(&mut s, "file.new", json!({"name": "Front"}));
    s
}

#[test]
fn the_users_own_calls_get_the_paths() {
    let mut s = two_files();
    let list = ex(&mut s, "doc.list", json!({}));
    assert_eq!(list[0]["path"], json!(SECRET));
    assert_eq!(list[1]["path"], json!(null), "a new file has none yet");
}

#[test]
fn an_agent_gets_names_and_ids_but_no_paths() {
    let mut s = two_files();
    let list = s.execute_as(Some("Claude"), "doc.list", &json!({})).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert!(list[0]["id"].is_u64() && list[0]["name"].is_string());
    assert!(list.as_array().unwrap().iter().all(|e| e.get("path").is_none()), "{list}");
    // An agent with no name of its own is an agent too.
    let list = s.execute_as(Some(""), "doc.list", &json!({})).unwrap();
    assert!(!list.to_string().contains(SECRET));
}
