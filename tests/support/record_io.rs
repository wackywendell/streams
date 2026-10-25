use std::path::PathBuf;
use streams_temper::yaml_store::records::RecordIoEvent;

/// Named read/lock/list attempts, separate from byte transfers and record replacements.
pub fn accesses(events: &[RecordIoEvent]) -> Vec<RecordIoEvent> {
    events
        .iter()
        .filter(|event| {
            matches!(
                event,
                RecordIoEvent::Read(_) | RecordIoEvent::Lock(_) | RecordIoEvent::List(_)
            )
        })
        .cloned()
        .collect()
}

pub fn bytes_read(events: &[RecordIoEvent]) -> usize {
    events
        .iter()
        .filter_map(|event| match event {
            RecordIoEvent::BytesRead(bytes) => Some(bytes),
            _ => None,
        })
        .sum()
}

pub fn writes(events: &[RecordIoEvent]) -> Vec<(PathBuf, usize)> {
    events
        .iter()
        .filter_map(|event| match event {
            RecordIoEvent::Written(path, bytes) => Some((path.clone(), *bytes)),
            _ => None,
        })
        .collect()
}
