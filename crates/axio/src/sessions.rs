//! Recording a session, resuming one, and listing what is on disk.

use super::*;

/// A readable age from a unix timestamp in seconds.
///
/// Nobody reads unix seconds, and this listing is the only way to find the id
/// `--resume` wants — with several sessions from one project the label repeats
/// and the timestamp is the sole disambiguator.
pub(crate) fn age(started: &str) -> String {
    let Ok(then) = started.parse::<u64>() else {
        return started.to_owned();
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(then);
    let secs = now.saturating_sub(then);
    match secs {
        0..=59 => "just now".to_owned(),
        60..=3_599 => format!("{}m ago", secs / 60),
        3_600..=86_399 => format!("{}h ago", secs / 3_600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

pub(crate) fn list_sessions() -> u8 {
    let store = SessionStore::new(state_dir().join("sessions"));
    let files = store.files();
    if files.is_empty() {
        println!("no sessions yet");
        return 0;
    }
    // One line read per file: a header carries everything a listing shows, so
    // listing never parses a transcript.
    for path in files.iter().take(50) {
        match record::read_header(path) {
            Ok(h) => {
                let label = h.label.as_deref().unwrap_or("");
                // The directory too: with sessions from several projects in one
                // state directory, the label is otherwise the only disambiguator
                // and labels repeat.
                let project = h
                    .cwd
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                println!(
                    "{}  {:>9}  {:<14}  {:<16}  {}",
                    &h.id.to_string()[..8],
                    age(&h.started),
                    h.model,
                    project,
                    label
                );
            }
            Err(_) => println!("{}  (unreadable)", path.display()),
        }
    }
    if files.len() > 50 {
        println!("… and {} more", files.len() - 50);
    }
    0
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Start recording a new session, unless asked not to.
pub(crate) fn new_recorder(
    cli: &Cli,
    store: &SessionStore,
    session: &Session,
    prompt: &str,
    notices: &mut Vec<Notice>,
) -> Recorder {
    if cli.ephemeral {
        return Recorder::Ephemeral;
    }
    let header = Header {
        version: SESSION_FORMAT_VERSION,
        protocol: axio_core::PROTOCOL_VERSION,
        id: session.id(),
        cwd: session.cwd().clone(),
        model: session.model().to_owned(),
        started: iso_now(),
        label: Some(label_from(prompt)),
        axio: env!("CARGO_PKG_VERSION").to_owned(),
    };
    match SessionFile::create(store.path_for(session.id()), &header) {
        Ok(file) => Recorder::File(file),
        Err(e) => {
            // Losing the log is not a reason to refuse to work.
            notices.push(Notice::warn(format!(
                "not recording this session ({e}); it will not be resumable"
            )));
            Recorder::Ephemeral
        }
    }
}

pub(crate) fn open_resumed(
    store: &SessionStore,
    needle: &str,
    model_override: Option<&str>,
    notices: &mut Vec<Notice>,
) -> Result<ResumedParts, String> {
    let id: SessionId = store.resolve(needle)?;
    let path = store.path_for(id);
    let loaded = record::load(&path).map_err(|e| format!("cannot resume {id}: {e}"))?;

    let mut session = loaded.session;
    notices.extend(loaded.notices);
    if loaded.degraded {
        notices.push(Notice::warn(
            "part of this session could not be read; the model is seeing a history with a hole in it",
        ));
    }
    if let Some(model) = model_override
        && model != session.model()
    {
        notices.push(Notice::warn(format!(
            "resuming under {model} instead of {}; earlier reasoning will not be replayed",
            session.model()
        )));
        session.adopt_model(model);
    }

    let recorder = match SessionFile::reopen(path) {
        Ok(file) => Recorder::File(file),
        Err(e) => {
            notices.push(Notice::warn(format!("continuing without recording ({e})")));
            Recorder::Ephemeral
        }
    };
    Ok((session, true, recorder))
}

pub(crate) fn iso_now() -> String {
    // Seconds since the epoch is not a date a human reads, but pulling a date
    // library into the binary for one string is not worth it either; the ULID
    // carries the authoritative timestamp.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

/// One line of the first prompt, so a listing is readable.
pub(crate) fn label_from(prompt: &str) -> String {
    let line = prompt.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let cleaned: String = line.chars().filter(|c| !c.is_control()).take(80).collect();
    cleaned.trim().to_owned()
}
