use std::time::Duration;

use helix_event::register_hook;
use helix_lsp::{lsp, MplsEditorDidChangeFocus, MplsEditorDidChangeFocusParams};
use helix_view::{
    events::DocumentFocusGained,
    handlers::{Handlers, MplsFocusEvent},
    DocumentId,
};
use tokio::time::Instant;

use crate::job;

const MPLS_FOCUS_DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Default)]
pub(super) struct MplsFocusHandler {
    doc: Option<DocumentId>,
}

impl helix_event::AsyncHook for MplsFocusHandler {
    type Event = MplsFocusEvent;

    fn handle_event(&mut self, event: Self::Event, _timeout: Option<Instant>) -> Option<Instant> {
        self.doc = Some(event.doc);
        Some(Instant::now() + MPLS_FOCUS_DEBOUNCE)
    }

    fn finish_debounce(&mut self) {
        let Some(doc_id) = self.doc.take() else {
            return;
        };

        job::dispatch_blocking(move |editor, _compositor| {
            // Check if config option is enabled
            if !editor.config().lsp.mpls_focus_notify {
                return;
            }

            let Some(doc) = editor.document(doc_id) else {
                return;
            };

            // Only send for markdown files
            if doc.language_id() != Some("markdown") {
                return;
            }

            // Get the document URI
            let Some(path) = doc.path() else {
                return;
            };
            let Ok(uri) = lsp::Url::from_file_path(path) else {
                return;
            };

            // Find the mpls language server
            let mpls_client = doc.language_servers().find(|ls| ls.name() == "mpls");
            let Some(client) = mpls_client else {
                return;
            };

            // Send the notification
            client.notify::<MplsEditorDidChangeFocus>(MplsEditorDidChangeFocusParams { uri });
        });
    }
}

pub(super) fn register_hooks(handlers: &Handlers) {
    let tx = handlers.mpls_focus.clone();
    register_hook!(move |event: &mut DocumentFocusGained<'_>| {
        helix_event::send_blocking(&tx, MplsFocusEvent { doc: event.doc });
        Ok(())
    });
}
