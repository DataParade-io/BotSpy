//! Steps for the Cursor workspace grouping (spec
//! 03_importers/cursor/workspace.feature).
//!
//! Every installation is synthetic and built here: a temporary directory
//! holding `User/globalStorage/state.vscdb` and one
//! `User/workspaceStorage/<id>/workspace.json` per workspace.

use crate::steps::timeline_steps::timeline_example;
use crate::steps::BotSpyWorld;
use botspy::adapters::cursor::{CursorSource, NO_WORKSPACE_PROJECT};
use botspy::snapshot::digest_files;
use botspy::Session;
use cucumber::{given, then, when};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct WorkspaceState {
    scratch: Option<tempfile::TempDir>,
    /// Workspace name to its 32-hex id.
    workspaces: BTreeMap<String, String>,
    /// Composers in the order the scenario added them, and the titles they
    /// were given.
    composers: Vec<Composer>,
    link: Option<PathBuf>,
    link_dir: Option<tempfile::TempDir>,
    digest_before: Option<String>,
    sessions: BTreeMap<String, Session>,
    summaries: BTreeMap<String, botspy::SessionSummary>,
}

#[derive(Debug)]
struct Composer {
    id: String,
    title: String,
    workspace_id: Option<String>,
    subagent: Option<String>,
}

fn scratch(state: &mut WorkspaceState) -> PathBuf {
    state
        .scratch
        .get_or_insert_with(|| tempfile::tempdir().expect("scratch dir"))
        .path()
        .to_path_buf()
}

fn workspace_dir(state: &mut WorkspaceState, id: &str) -> PathBuf {
    let dir = scratch(state).join("Cursor/User/workspaceStorage").join(id);
    std::fs::create_dir_all(&dir).expect("workspace dir");
    dir
}

fn add_workspace(world: &mut BotSpyWorld, name: &str, json: Value) {
    let state = &mut world.cursor_ws;
    let id = format!("{:032x}", state.workspaces.len() + 1);
    let dir = workspace_dir(state, &id);
    std::fs::write(dir.join("workspace.json"), json.to_string()).expect("write workspace.json");
    state.workspaces.insert(name.to_string(), id);
}

fn store_path(state: &mut WorkspaceState) -> PathBuf {
    scratch(state).join("Cursor/User/globalStorage/state.vscdb")
}

#[given(regex = r#"^a Cursor installation with the workspace "([^"]+)" at the folder "([^"]+)"$"#)]
fn workspace_at_folder(world: &mut BotSpyWorld, name: String, uri: String) {
    add_workspace(world, &name, json!({"folder": uri}));
}

#[given(
    regex = r#"^a Cursor installation with the workspace "([^"]+)" in the code-workspace file "([^"]+)"$"#
)]
fn workspace_in_file(world: &mut BotSpyWorld, name: String, uri: String) {
    add_workspace(world, &name, json!({"workspace": uri}));
}

fn add_composer(
    world: &mut BotSpyWorld,
    id: String,
    title: String,
    workspace_id: Option<String>,
    subagent: Option<String>,
) {
    world.cursor_ws.composers.push(Composer {
        id,
        title,
        workspace_id,
        subagent,
    });
}

fn workspace_id_of(world: &BotSpyWorld, name: &str) -> String {
    world
        .cursor_ws
        .workspaces
        .get(name)
        .unwrap_or_else(|| panic!("no workspace named {name}"))
        .clone()
}

#[given(regex = r#"^a composer "([^"]+)" titled "([^"]+)" in the workspace "([^"]+)"$"#)]
fn composer_in_workspace(world: &mut BotSpyWorld, id: String, title: String, workspace: String) {
    let workspace_id = workspace_id_of(world, &workspace);
    add_composer(world, id, title, Some(workspace_id), None);
}

#[given(regex = r#"^a composer "([^"]+)" titled "([^"]+)" with no workspace$"#)]
fn composer_without_workspace(world: &mut BotSpyWorld, id: String, title: String) {
    add_composer(world, id, title, None, None);
}

#[given(regex = r#"^a composer "([^"]+)" titled "([^"]+)" in the workspace id "([^"]+)"$"#)]
fn composer_in_workspace_id(world: &mut BotSpyWorld, id: String, title: String, workspace: String) {
    add_composer(world, id, title, Some(workspace), None);
}

#[given(regex = r#"^a composer "([^"]+)" titled "([^"]+)" in a workspace that names no folder$"#)]
fn composer_in_folderless_workspace(world: &mut BotSpyWorld, id: String, title: String) {
    add_workspace(world, &format!("folderless-{id}"), json!({}));
    let workspace_id = workspace_id_of(world, &format!("folderless-{id}"));
    add_composer(world, id, title, Some(workspace_id), None);
}

#[given(
    regex = r#"^a composer "([^"]+)" titled "([^"]+)" in the workspace "([^"]+)" that is a sub-agent of type "([^"]+)"$"#
)]
fn typed_subagent(
    world: &mut BotSpyWorld,
    id: String,
    title: String,
    workspace: String,
    kind: String,
) {
    let workspace_id = workspace_id_of(world, &workspace);
    add_composer(world, id, title, Some(workspace_id), Some(kind));
}

#[given(
    regex = r#"^a composer "([^"]+)" titled "([^"]+)" in the workspace "([^"]+)" that is a sub-agent without a type$"#
)]
fn untyped_subagent(world: &mut BotSpyWorld, id: String, title: String, workspace: String) {
    let workspace_id = workspace_id_of(world, &workspace);
    add_composer(world, id, title, Some(workspace_id), Some(String::new()));
}

#[given(regex = r#"^the store is reached through a symbolic link in another directory$"#)]
fn store_behind_link(world: &mut BotSpyWorld) {
    let other = tempfile::tempdir().expect("link dir");
    world.cursor_ws.link = Some(other.path().join("state.vscdb"));
    world.cursor_ws.link_dir = Some(other);
}

#[given(
    regex = r#"^a Cursor KV store whose "composerHeaders" key lists a composer "([^"]+)" titled "([^"]+)"$"#
)]
fn old_layout_store(world: &mut BotSpyWorld, id: String, title: String) {
    let path = store_path(&mut world.cursor_ws);
    std::fs::create_dir_all(path.parent().expect("store dir")).expect("store dir");
    let conn = open(&path);
    put(
        &conn,
        "composerHeaders",
        &json!([{"composerId": id, "lastUpdatedAt": 1, "name": title}]),
    );
    put(
        &conn,
        &format!("composerData:{id}"),
        &json!({"name": title, "fullConversationHeadersOnly": []}),
    );
    world.cursor_ws.composers.clear();
}

fn open(path: &Path) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open(path).expect("open Cursor KV store");
    conn.execute(
        "CREATE TABLE IF NOT EXISTS cursorDiskKV (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        [],
    )
    .expect("create cursorDiskKV table");
    conn
}

fn put(conn: &rusqlite::Connection, key: &str, value: &Value) {
    conn.execute(
        "INSERT OR REPLACE INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
        rusqlite::params![key, value.to_string()],
    )
    .expect("write KV value");
}

/// Write the composers added so far into a store with the current
/// `composerHeaders` table, unless an older-layout store was written.
fn write_store(state: &mut WorkspaceState) -> PathBuf {
    let path = store_path(state);
    if state.composers.is_empty() {
        return path;
    }
    std::fs::create_dir_all(path.parent().expect("store dir")).expect("store dir");
    let conn = open(&path);
    conn.execute(
        "CREATE TABLE composerHeaders (composerId TEXT PRIMARY KEY, workspaceId TEXT, \
         createdAt INTEGER, lastUpdatedAt INTEGER, isArchived INTEGER, isSubagent INTEGER, \
         recency INTEGER, checkpointAt INTEGER, value TEXT, subagentTypeName TEXT)",
        [],
    )
    .expect("create composerHeaders table");
    for composer in &state.composers {
        conn.execute(
            "INSERT INTO composerHeaders (composerId, workspaceId, createdAt, lastUpdatedAt, \
             isSubagent, value, subagentTypeName) VALUES (?1, ?2, 1, 2, ?3, ?4, ?5)",
            rusqlite::params![
                composer.id,
                composer.workspace_id,
                composer.subagent.is_some() as i64,
                json!({"composerId": composer.id, "name": composer.title}).to_string(),
                composer.subagent.as_deref().filter(|kind| !kind.is_empty())
            ],
        )
        .expect("insert composer header");
        put(
            &conn,
            &format!("composerData:{}", composer.id),
            &json!({"name": composer.title, "fullConversationHeadersOnly": [{"bubbleId": "b1"}]}),
        );
        put(
            &conn,
            &format!("bubbleId:{}:b1", composer.id),
            &json!({"type": 1, "text": "hello"}),
        );
    }
    path
}

fn workspace_files(state: &WorkspaceState) -> Vec<PathBuf> {
    state
        .workspaces
        .values()
        .map(|id| {
            state
                .scratch
                .as_ref()
                .expect("scratch")
                .path()
                .join("Cursor/User/workspaceStorage")
                .join(id)
                .join("workspace.json")
        })
        .collect()
}

#[when(regex = r#"^the Cursor importer reads the installation$"#)]
fn read_installation(world: &mut BotSpyWorld) {
    let state = &mut world.cursor_ws;
    let store = write_store(state);
    let files = workspace_files(state);
    let paths: Vec<&Path> = files.iter().map(PathBuf::as_path).collect();
    state.digest_before = Some(digest_files(&paths));
    let opened = match &state.link {
        Some(link) => {
            std::os::unix::fs::symlink(&store, link).expect("link the store");
            link.clone()
        }
        None => store,
    };
    let source = CursorSource::new(opened);
    let (summaries, _) = source.discover();
    for summary in summaries {
        let session = source
            .extract(&summary.id)
            .expect("composer should extract")
            .session;
        let mut session = session;
        // One plain half hour, so the timeline can draw the lane.
        session.started_at = "2026-10-01T09:00:00Z".to_string();
        session.last_activity_at = "2026-10-01T09:30:00Z".to_string();
        state.sessions.insert(summary.id.clone(), session);
        state.summaries.insert(summary.id.clone(), summary);
    }
}

fn session<'a>(world: &'a BotSpyWorld, id: &str) -> &'a Session {
    world
        .cursor_ws
        .sessions
        .get(id)
        .unwrap_or_else(|| panic!("no session for composer {id}"))
}

fn id_list(text: &str) -> Vec<String> {
    text.split('"')
        .enumerate()
        .filter(|(i, _)| i % 2 == 1)
        .map(|(_, id)| id.to_string())
        .collect()
}

#[then(regex = r#"^the composer "([^"]+)" has the working directory "([^"]+)"$"#)]
fn has_working_directory(world: &mut BotSpyWorld, id: String, cwd: String) {
    assert_eq!(
        session(world, &id).metadata.cwd.as_deref(),
        Some(cwd.as_str())
    );
    assert_eq!(
        world.cursor_ws.summaries[&id].metadata.cwd.as_deref(),
        Some(cwd.as_str()),
        "discovery and extraction disagree"
    );
}

#[then(regex = r#"^the composer "([^"]+)" has the project id of the workspace "([^"]+)"$"#)]
fn has_workspace_project(world: &mut BotSpyWorld, id: String, workspace: String) {
    let expected = workspace_id_of(world, &workspace);
    assert_eq!(session(world, &id).project_id, expected);
    assert_eq!(world.cursor_ws.summaries[&id].project_id, expected);
}

#[then(regex = r#"^the composers "([^"]+)" and "([^"]+)" share one project$"#)]
fn share_a_project(world: &mut BotSpyWorld, a: String, b: String) {
    assert_eq!(session(world, &a).project_id, session(world, &b).project_id);
}

#[then(regex = r#"^the composers "([^"]+)" and "([^"]+)" are in different projects$"#)]
fn different_projects(world: &mut BotSpyWorld, a: String, b: String) {
    assert_ne!(session(world, &a).project_id, session(world, &b).project_id);
}

#[then(regex = r#"^the composer "([^"]+)" is in the project "([^"]+)"$"#)]
fn in_project(world: &mut BotSpyWorld, id: String, project: String) {
    assert_eq!(session(world, &id).project_id, project);
    assert_eq!(world.cursor_ws.summaries[&id].project_id, project);
}

#[then(regex = r#"^the composers ((?:"[^"]+"(?:, | and )?)+) are in the project "([^"]+)"$"#)]
fn all_in_project(world: &mut BotSpyWorld, ids: String, project: String) {
    assert_eq!(project, NO_WORKSPACE_PROJECT);
    for id in id_list(&ids) {
        in_project(world, id, project.clone());
    }
}

#[then(regex = r#"^the composers ((?:"[^"]+"(?:, | and )?)+) have no working directory$"#)]
fn all_without_cwd(world: &mut BotSpyWorld, ids: String) {
    for id in id_list(&ids) {
        assert_eq!(session(world, &id).metadata.cwd, None, "{id}");
        assert_eq!(world.cursor_ws.summaries[&id].metadata.cwd, None, "{id}");
    }
}

#[then(regex = r#"^no project id is a title$"#)]
fn no_project_is_a_title(world: &mut BotSpyWorld) {
    let titles: Vec<&str> = world
        .cursor_ws
        .composers
        .iter()
        .map(|composer| composer.title.as_str())
        .collect();
    for (id, session) in &world.cursor_ws.sessions {
        assert!(
            !titles.contains(&session.project_id.as_str()),
            "composer {id} has a title as its project"
        );
        assert!(
            titles
                .iter()
                .all(|title| !session.project_id.contains(title)),
            "composer {id} has a project that contains a title"
        );
    }
}

#[then(regex = r#"^the composer "([^"]+)" keeps the title "([^"]+)" in its metadata$"#)]
fn keeps_title(world: &mut BotSpyWorld, id: String, title: String) {
    assert_eq!(
        session(world, &id).metadata.title.as_deref(),
        Some(title.as_str())
    );
}

#[then(regex = r#"^the workspace files were never mutated$"#)]
fn workspace_files_unchanged(world: &mut BotSpyWorld) {
    let files = workspace_files(&world.cursor_ws);
    let paths: Vec<&Path> = files.iter().map(PathBuf::as_path).collect();
    assert_eq!(
        Some(digest_files(&paths)),
        world.cursor_ws.digest_before,
        "a workspace file changed"
    );
}

#[then(regex = r#"^the composer "([^"]+)" is a sub-agent of kind "([^"]+)" with no parent$"#)]
fn is_subagent(world: &mut BotSpyWorld, id: String, kind: String) {
    let session = session(world, &id);
    let info = session.subagent.as_ref().expect("a sub-agent");
    assert_eq!(info.kind, kind);
    assert_eq!(info.nesting_depth, None);
    assert_eq!(session.parent_id, None);
}

#[then(regex = r#"^the composer "([^"]+)" is not a sub-agent$"#)]
fn is_not_subagent(world: &mut BotSpyWorld, id: String) {
    assert_eq!(session(world, &id).subagent, None);
}

fn render(world: &mut BotSpyWorld, args: &[String]) {
    let sessions: Vec<Session> = world.cursor_ws.sessions.values().cloned().collect();
    world.timeline.html =
        timeline_example::page_for(args, Some(sessions)).expect("the example renders");
}

#[when(regex = r#"^the timeline example renders the Cursor sessions$"#)]
fn render_plain(world: &mut BotSpyWorld) {
    render(world, &[]);
}

#[when(regex = r#"^the timeline example renders the Cursor sessions with "([^"]*)"$"#)]
fn render_with(world: &mut BotSpyWorld, options: String) {
    let args: Vec<String> = options.split_whitespace().map(str::to_string).collect();
    render(world, &args);
}

#[then(regex = r#"^the timeline has a group of lanes for the project "([^"]+)"$"#)]
fn has_project_group(world: &mut BotSpyWorld, project: String) {
    let needle = format!("data-project=\"{project}\"");
    assert!(
        world.timeline.html.contains(&needle),
        "no lane in {project}"
    );
}

#[then(regex = r#"^the timeline draws the composer "([^"]+)" as a sub-agent lane$"#)]
fn draws_subagent_lane(world: &mut BotSpyWorld, id: String) {
    let html = &world.timeline.html;
    let at = html
        .find(&format!("data-session=\"{id}\""))
        .expect("the composer has a lane");
    let tag_end = html[at..].find('>').expect("lane tag");
    assert!(
        html[at..at + tag_end].contains("data-relation=\"subagent\""),
        "the lane is not drawn as a sub-agent"
    );
}
