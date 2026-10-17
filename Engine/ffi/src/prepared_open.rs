//! One-file interactive import hands sensor samples to the selected photo's first full development.

use lightcraft_engine::Session;
use lightcraft_engine::catalog::Source;
use lightcraft_engine::files::{PreparedOpen, PreparedSources};
use serde_json::{Value, json};

use crate::status::Failure;

/// Resolve the selected catalog identity to the file whose preparation may be retained.
pub(crate) fn active_path(session: &Session) -> Option<String> {
    match &session.catalog.photo(session.active()?)?.source {
        Source::File { path } => Some(path.clone()),
        Source::Demo { .. } => None,
    }
}

/// Only a single existing file is an interactive preparation candidate; directory imports are not.
pub(crate) fn before(sources: &PreparedSources, command: &str, params: &Value) -> Option<PreparedOpen> {
    match command {
        "library.import" => {
            sources.clear();
            let paths = params.get("paths")?.as_array()?;
            if paths.len() != 1 {
                return None;
            }
            let path = std::path::absolute(paths[0].as_str()?).ok()?;
            path.is_file()
                .then(|| sources.begin_open(&path.to_string_lossy()))
        }
        "photo.reload" | "library.close" => {
            sources.clear();
            None
        }
        _ => None,
    }
}

/// Failed imports cancel their own token; selection commits only the catalog's actual active file.
pub(crate) fn after(
    sources: &PreparedSources,
    session: &Session,
    command: &str,
    previous_path: Option<&str>,
    token: Option<PreparedOpen>,
    result: &Result<Value, Failure>,
) {
    if let Some(token) = token {
        let has_photo = result.as_ref().is_ok_and(|value| {
            ["imported", "restored", "duplicates"]
                .into_iter()
                .any(|key| value[key].as_array().is_some_and(|ids| !ids.is_empty()))
        });
        if !has_photo {
            sources.cancel_open(token);
        }
    }
    let selected = active_path(session);
    if result.is_ok() && (command == "library.select" || selected.as_deref() != previous_path) {
        sources.select(selected.as_deref());
    }
}

/// Report preparation owners separately from decoded RGB sources and resident GPU stages.
pub(crate) fn memory(sources: &PreparedSources) -> Value {
    let stats = sources.stats();
    json!({
        "retainedBytes": stats.retained_bytes,
        "sampleDecodes": stats.sample_decodes,
        "preparedHits": stats.prepared_hits,
        "contentMisses": stats.content_misses,
        "oversizeMisses": stats.oversize_misses,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_single_file_imports_start_a_provisional_preparation() {
        let sources = PreparedSources::new(128 << 20);
        let file = std::env::current_exe().unwrap();
        assert!(before(&sources, "library.import", &json!({"paths":[file]})).is_some());
        assert!(sources.pending_timeout().is_some());
        for paths in [json!([file, file]), json!([std::env::temp_dir()]), json!([])] {
            assert!(before(&sources, "library.import", &json!({"paths":paths})).is_none());
            assert!(sources.pending_timeout().is_none());
        }
    }

    #[test]
    fn failed_imports_and_selection_without_an_active_file_cancel_preparation() {
        let sources = PreparedSources::new(128 << 20);
        let session = Session::new();
        let token = sources.begin_open("failed.nef");
        after(
            &sources,
            &session,
            "library.import",
            None,
            Some(token),
            &Ok(json!({"imported":[],"failed":[["failed.nef","bad"]]})),
        );
        assert!(sources.pending_timeout().is_none());
        sources.begin_open("unselected.nef");
        after(&sources, &session, "library.select", None, None, &Ok(json!({})));
        assert!(sources.pending_timeout().is_none());
    }

    #[test]
    fn an_old_import_failure_cannot_cancel_a_new_open() {
        let sources = PreparedSources::new(128 << 20);
        let token = sources.begin_open("old.nef");
        let current = sources.begin_open("new.nef");
        after(
            &sources,
            &Session::new(),
            "library.import",
            None,
            Some(token),
            &Err(Failure::invalid("cancelled")),
        );
        assert!(sources.confirm_open(current));
    }
}
