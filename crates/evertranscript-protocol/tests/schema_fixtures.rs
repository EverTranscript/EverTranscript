//! The committed protocol schema is the contract; drift is a failing build.
//!
//! Regenerate with `cargo test -p evertranscript-protocol` after an
//! intentional protocol change, then commit the diff (`schema/` here and the
//! ts-rs `bindings/` written by the same run). Reviewers read that diff as
//! the protocol change itself.

use std::path::PathBuf;

fn schema_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schema")
}

fn check_or_write(name: &str, generated: String) {
    let path = schema_dir().join(name);
    std::fs::create_dir_all(schema_dir()).expect("create schema dir");

    let regenerate = std::env::var_os("EVERTRANSCRIPT_REGENERATE_FIXTURES").is_some();
    match std::fs::read_to_string(&path) {
        Ok(committed) if committed == generated => {}
        Ok(_committed) if regenerate => {
            std::fs::write(&path, generated).expect("write fixture");
        }
        Ok(_committed) => {
            std::fs::write(path.with_extension("json.actual"), &generated).expect("write actual");
            panic!(
                "protocol schema drifted from the committed fixture ({name}).\n\
                 The generated schema was written beside it as {name}.actual.\n\
                 If the change is intentional, re-run with \
                 EVERTRANSCRIPT_REGENERATE_FIXTURES=1 and commit the result."
            );
        }
        Err(_) => {
            std::fs::write(&path, generated).expect("write fixture");
        }
    }
}

#[test]
fn client_request_schema_matches_the_committed_fixture() {
    let schema = schemars::schema_for!(evertranscript_protocol::ClientRequest);
    let generated = format!(
        "{}\n",
        serde_json::to_string_pretty(&schema).expect("serialize schema")
    );
    check_or_write("client-request.schema.json", generated);
}

#[test]
fn server_notification_schema_matches_the_committed_fixture() {
    let schema = schemars::schema_for!(evertranscript_protocol::ServerNotification);
    let generated = format!(
        "{}\n",
        serde_json::to_string_pretty(&schema).expect("serialize schema")
    );
    check_or_write("server-notification.schema.json", generated);
}

#[test]
fn envelope_schema_matches_the_committed_fixture() {
    let schema = schemars::schema_for!(evertranscript_protocol::JsonRpcMessage);
    let generated = format!(
        "{}\n",
        serde_json::to_string_pretty(&schema).expect("serialize schema")
    );
    check_or_write("envelope.schema.json", generated);
}
