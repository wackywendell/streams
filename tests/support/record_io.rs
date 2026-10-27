use std::path::PathBuf;
use streams::yaml_store::records::RecordIoEvent;

/// Named read/lock/list attempts, separate from byte transfers and record replacements.
pub fn accesses(events: &[RecordIoEvent]) -> Vec<RecordIoEvent> {
    events
        .iter()
        .filter(|event| event.is_access())
        .cloned()
        .collect()
}

pub fn bytes_read(events: &[RecordIoEvent]) -> usize {
    events.iter().map(RecordIoEvent::bytes_read).sum()
}

pub fn writes(events: &[RecordIoEvent]) -> Vec<(PathBuf, usize)> {
    events
        .iter()
        .filter_map(RecordIoEvent::written)
        .map(|(path, bytes)| (path.to_owned(), bytes))
        .collect()
}
