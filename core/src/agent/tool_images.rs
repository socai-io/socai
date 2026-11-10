//! Bounded, run-local visual context. Archived/uploaded file previews can be
//! compared across steps; browser screenshots still expire after one request.

use std::collections::VecDeque;

use super::llm::ToolResultContent;

const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_IMAGES: usize = 16;
const MAX_IMAGE_BYTES: usize = 6 * 1024 * 1024;

struct Entry {
    call_id: String,
    image: ToolResultContent,
    bytes: usize,
    persistent: bool,
}

#[derive(Default)]
pub(crate) struct ToolImageWindow {
    entries: VecDeque<Entry>,
    bytes: usize,
}

impl ToolImageWindow {
    pub(crate) fn insert(
        &mut self,
        call_id: &str,
        content: &[ToolResultContent],
        persistent: bool,
    ) {
        // A tool-call ID identifies one result, not a growing image album.
        self.remove_where(|entry| entry.call_id == call_id);
        for image in content {
            let ToolResultContent::Image { data, .. } = image else {
                continue;
            };
            if data.len() > MAX_IMAGE_BYTES {
                continue;
            }
            let bytes = data.len();
            self.entries.push_back(Entry {
                call_id: call_id.to_string(),
                image: image.clone(),
                bytes,
                persistent,
            });
            self.bytes += bytes;
            while self.bytes > MAX_BYTES || self.entries.len() > MAX_IMAGES {
                if let Some(oldest) = self.entries.pop_front() {
                    self.bytes -= oldest.bytes;
                }
            }
        }
    }

    pub(crate) fn images_for<'a>(
        &'a self,
        call_id: &'a str,
    ) -> impl Iterator<Item = &'a ToolResultContent> {
        self.entries
            .iter()
            .filter(move |entry| entry.call_id == call_id)
            .map(|entry| &entry.image)
    }

    pub(crate) fn after_request(&mut self) {
        self.remove_where(|entry| !entry.persistent);
    }

    fn remove_where(&mut self, mut remove: impl FnMut(&Entry) -> bool) {
        let mut bytes = 0;
        self.entries.retain(|entry| {
            if remove(entry) {
                return false;
            }
            bytes += entry.bytes;
            true
        });
        self.bytes = bytes;
    }
}
