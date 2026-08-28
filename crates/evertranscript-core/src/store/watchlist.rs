//! Reading and writing the Watchlist.
//!
//! Seeded by migration 5 rather than defaulted in code: an empty list means
//! the Operator removed every row and is entitled to that, not that the
//! defaults should quietly come back on the next start.

use anyhow::Result;
use rusqlite::Connection;

use crate::detect::watchlist::EntryKind;
use crate::detect::watchlist::Watchlist;
use crate::detect::watchlist::WatchlistEntry;

fn kind_str(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::Process => "process",
        EntryKind::BrowserMeetings => "browserMeetings",
    }
}

fn kind_from(text: &str) -> EntryKind {
    match text {
        "browserMeetings" => EntryKind::BrowserMeetings,
        _ => EntryKind::Process,
    }
}

pub fn load(connection: &Connection) -> Result<Watchlist> {
    let mut statement = connection.prepare("SELECT id, name, kind FROM watchlist ORDER BY id")?;
    let entries = statement
        .query_map([], |row| {
            Ok(WatchlistEntry {
                id: row.get(0)?,
                name: row.get(1)?,
                kind: kind_from(&row.get::<_, String>(2)?),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Watchlist::from_entries(entries))
}

/// Adds a row. Returns false when it was already there.
pub fn add(connection: &Connection, entry: &WatchlistEntry) -> Result<bool> {
    let changed = connection.execute(
        "INSERT OR IGNORE INTO watchlist (id, name, kind) VALUES (?1, ?2, ?3)",
        rusqlite::params![entry.id, entry.name, kind_str(entry.kind)],
    )?;
    Ok(changed > 0)
}

/// Removes a row. Returns false when it was not there.
pub fn remove(connection: &Connection, id: &str) -> Result<bool> {
    let changed = connection.execute("DELETE FROM watchlist WHERE id = ?1", [id])?;
    Ok(changed > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::AppIdentity;

    fn database() -> Connection {
        let mut connection = Connection::open_in_memory().expect("open");
        crate::store::schema::configure(&connection).expect("configure");
        crate::store::schema::migrate(&mut connection).expect("migrate");
        connection
    }

    #[test]
    fn a_fresh_installation_watches_what_adr_0030_ships() {
        let list = load(&database()).expect("load");
        for watched in ["us.zoom.xos", "com.microsoft.teams2", "com.tencent.meeting"] {
            assert!(list.watches(&AppIdentity::bare(watched)), "{watched}");
        }
        assert!(
            list.watches(&AppIdentity::bare("com.google.Chrome")),
            "browsers"
        );
        assert!(
            !list.watches(&AppIdentity::bare("com.tencent.xinWeChat")),
            "WeChat ships suggested, not watched"
        );
    }

    #[test]
    fn an_operator_who_empties_the_list_keeps_it_empty() {
        // The reason the defaults are seeded in a migration rather than
        // supplied by code when the table is empty: "I removed everything"
        // and "this is a fresh install" must not look the same.
        let connection = database();
        for entry in load(&connection).expect("load").entries().to_vec() {
            remove(&connection, &entry.id).expect("remove");
        }
        let list = load(&connection).expect("reload");
        assert!(
            list.entries().is_empty(),
            "still empty, got {:?}",
            list.entries()
        );
        assert!(!list.watches(&AppIdentity::bare("us.zoom.xos")));
    }

    #[test]
    fn adding_is_idempotent_and_removing_reports_honestly() {
        let connection = database();
        let webex = WatchlistEntry::process("com.webex.meetingmanager", "Webex");
        assert!(add(&connection, &webex).expect("add"));
        assert!(
            !add(&connection, &webex).expect("add again"),
            "already there"
        );
        assert!(
            load(&connection)
                .expect("load")
                .watches(&AppIdentity::bare(&webex.id))
        );
        assert!(remove(&connection, &webex.id).expect("remove"));
        assert!(
            !remove(&connection, &webex.id).expect("remove again"),
            "gone already"
        );
    }
}
