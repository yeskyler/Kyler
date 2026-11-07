use super::manager::Inner;
use crate::sandbox::ProcessChunk;

use kyler_protocol::api::processes::ProcessOutput;

pub(super) fn publish(inner: &Inner, id: &str, session: &str, chunk: ProcessChunk) {
    let mut entries = inner.entries.lock().unwrap();
    if let Some(entry) = entries.get_mut(id) {
        entry.info.cursor += 1;
        let output = ProcessOutput {
            process_id: id.into(),
            session_id: session.into(),
            cursor: entry.info.cursor,
            stream: chunk.stream,
            text: chunk.text,
        };
        entry.info.output.push(output.clone());
        while entry
            .info
            .output
            .iter()
            .map(|chunk| chunk.text.len())
            .sum::<usize>()
            > 64000
        {
            entry.info.output.remove(0);
        }
        entry.info.oldest_cursor = entry
            .info
            .output
            .first()
            .map(|chunk| chunk.cursor)
            .unwrap_or(entry.info.cursor);
        let _ = inner.events.send(output);
    }
}
