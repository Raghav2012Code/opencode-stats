//! Putting the share card where the user can paste it.
//!
//! One seam: [`perform_share`] decides what goes on the clipboard, what is written to
//! disk and what the user is told, given a way to open a clipboard. Tests drive it with
//! a fake sink, so the suite never touches the real clipboard.

use std::path::Path;
use std::path::PathBuf;
use std::thread::sleep;
use std::time::Duration;

use color_eyre::eyre::Result;
use image::RgbaImage;
use ratatui::buffer::Buffer;

use crate::ui::export::render_share_card;
use crate::ui::theme::Theme;

/// Windows hands the clipboard to one process at a time, so opening it or writing to it
/// fails while another app holds it. That is transient, so try a few times.
const ATTEMPTS: usize = 3;
const RETRY_DELAY: Duration = Duration::from_millis(40);

/// Where a share card can be put.
pub trait ClipboardSink {
    fn set_image(&mut self, image: &RgbaImage) -> Result<()>;
    fn set_text(&mut self, text: &str) -> Result<()>;
}

/// The system clipboard. Note that an image and a text summary cannot both be placed:
/// setting one replaces the other, so they are alternatives rather than complements.
pub struct SystemClipboard(arboard::Clipboard);

impl SystemClipboard {
    pub fn open() -> Result<Self> {
        Ok(Self(arboard::Clipboard::new()?))
    }
}

impl ClipboardSink for SystemClipboard {
    fn set_image(&mut self, image: &RgbaImage) -> Result<()> {
        self.0.set_image(arboard::ImageData {
            width: image.width() as usize,
            height: image.height() as usize,
            bytes: std::borrow::Cow::Owned(image.clone().into_raw()),
        })?;
        Ok(())
    }

    fn set_text(&mut self, text: &str) -> Result<()> {
        self.0.set_text(text.to_string())?;
        Ok(())
    }
}

/// What reached the clipboard.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClipboardContent {
    Image,
    /// The image could not be placed, so the summary was copied instead.
    Text {
        reason: String,
    },
    Nothing {
        error: String,
    },
}

/// What happened to the PNG written beside the exports, so the card is still usable when
/// a paste target refuses the image formats.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PngArtifact {
    Written(String),
    Failed(String),
    /// No card to write, because rendering it failed before anything else.
    Skipped,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShareOutcome {
    pub clipboard: ClipboardContent,
    pub png: PngArtifact,
}

impl ShareOutcome {
    /// One line for the footer: what reaches a paste, and where the file is.
    pub fn message(&self) -> String {
        let head = match &self.clipboard {
            ClipboardContent::Image => "Copied share card to clipboard".to_string(),
            ClipboardContent::Text { reason } => {
                format!("Copied text summary instead ({reason})")
            }
            ClipboardContent::Nothing { error } => format!("Copy failed: {error}"),
        };

        match &self.png {
            PngArtifact::Written(path) => format!("{head} | saved {path}"),
            PngArtifact::Failed(error) => format!("{head} | PNG write failed: {error}"),
            PngArtifact::Skipped => head,
        }
    }
}

pub struct ShareJob {
    pub buffer: Buffer,
    pub theme: Theme,
    pub summary: String,
    pub png_path: PathBuf,
}

/// Renders the card, writes it to disk, puts it on the clipboard, and reports what
/// happened. The PNG is written even when the clipboard path fails, so a copy never
/// leaves the user with nothing.
pub fn perform_share<F>(job: ShareJob, mut open: F) -> ShareOutcome
where
    F: FnMut() -> Result<Box<dyn ClipboardSink>>,
{
    let (image, render_error) = match render_share_card(&job.buffer, &job.theme) {
        Ok(image) => (Some(image), None),
        Err(err) => (None, Some(err.to_string())),
    };

    let png = match &image {
        Some(image) => write_png(image, &job.png_path),
        None => PngArtifact::Skipped,
    };

    let summary_text = |reason: String| ClipboardContent::Text { reason };
    let clipboard = match &image {
        Some(image) => match place(&mut open, |sink| sink.set_image(image)) {
            Ok(()) => ClipboardContent::Image,
            Err(image_error) => match place(&mut open, |sink| sink.set_text(&job.summary)) {
                Ok(()) => summary_text(format!("image failed: {image_error}")),
                Err(text_error) => ClipboardContent::Nothing {
                    error: format!("{image_error}; then {text_error}"),
                },
            },
        },
        None => {
            let reason =
                render_error.unwrap_or_else(|| "the share card could not be rendered".to_string());
            match place(&mut open, |sink| sink.set_text(&job.summary)) {
                Ok(()) => summary_text(reason),
                Err(text_error) => ClipboardContent::Nothing {
                    error: format!("{reason}; {text_error}"),
                },
            }
        }
    };

    ShareOutcome { clipboard, png }
}

/// Opens the clipboard and applies one write, retrying transient failures and keeping
/// the last error so the user is told what actually went wrong.
fn place<F, A>(open: &mut F, mut action: A) -> std::result::Result<(), String>
where
    F: FnMut() -> Result<Box<dyn ClipboardSink>>,
    A: FnMut(&mut dyn ClipboardSink) -> Result<()>,
{
    let mut last_error = String::new();
    for attempt in 1..=ATTEMPTS {
        match open().and_then(|mut sink| action(sink.as_mut())) {
            Ok(()) => return Ok(()),
            Err(err) => last_error = err.to_string(),
        }
        if attempt < ATTEMPTS {
            sleep(RETRY_DELAY);
        }
    }
    Err(last_error)
}

fn write_png(image: &RgbaImage, path: &Path) -> PngArtifact {
    match image.save(path) {
        Ok(()) => PngArtifact::Written(path.display().to_string()),
        Err(err) => PngArtifact::Failed(err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::time::{SystemTime, UNIX_EPOCH};

    use color_eyre::eyre::{Result, eyre};
    use image::RgbaImage;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Style;

    use super::{
        ClipboardContent, ClipboardSink, PngArtifact, ShareJob, ShareOutcome, perform_share,
    };
    use crate::ui::theme::Theme;

    #[derive(Default)]
    struct FakeState {
        image_calls: usize,
        opens: usize,
        images: Vec<(u32, u32)>,
        texts: Vec<String>,
        fail_image_calls: usize,
        fail_text_calls: usize,
    }

    struct FakeSink {
        state: Rc<RefCell<FakeState>>,
    }

    impl ClipboardSink for FakeSink {
        fn set_image(&mut self, image: &RgbaImage) -> Result<()> {
            let mut state = self.state.borrow_mut();
            state.image_calls += 1;
            if state.fail_image_calls > 0 {
                state.fail_image_calls -= 1;
                return Err(eyre!("the clipboard is held by another process"));
            }
            state.images.push((image.width(), image.height()));
            Ok(())
        }

        fn set_text(&mut self, text: &str) -> Result<()> {
            let mut state = self.state.borrow_mut();
            if state.fail_text_calls > 0 {
                state.fail_text_calls -= 1;
                return Err(eyre!("the clipboard is held by another process"));
            }
            state.texts.push(text.to_string());
            Ok(())
        }
    }

    fn a_card_buffer() -> Buffer {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 24, 4));
        buffer.set_string(0, 0, "oc-stats card", Style::default());
        buffer
    }

    fn temp_dir(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("oc-stats-share-{label}-{nonce}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn job(dir: &Path, timestamp: i64) -> ShareJob {
        ShareJob {
            buffer: a_card_buffer(),
            theme: Theme::builtin_dark(),
            summary: "oc-stats all time: 30 tokens".to_string(),
            png_path: dir.join(format!("oc-stats-all-{timestamp}.png")),
        }
    }

    /// Runs a share whose clipboard behaves as described, and hands back the outcome plus
    /// what the fake clipboard saw.
    fn share(
        dir: &Path,
        timestamp: i64,
        configure: impl FnOnce(&mut FakeState),
    ) -> (ShareOutcome, FakeState) {
        let state = Rc::new(RefCell::new(FakeState::default()));
        configure(&mut state.borrow_mut());
        let outcome = perform_share(job(dir, timestamp), || {
            let mut shared = state.borrow_mut();
            shared.opens += 1;
            drop(shared);
            Ok(Box::new(FakeSink {
                state: Rc::clone(&state),
            }) as Box<dyn ClipboardSink>)
        });
        let snapshot_state = state.borrow();
        let observed = FakeState {
            image_calls: snapshot_state.image_calls,
            opens: snapshot_state.opens,
            images: snapshot_state.images.clone(),
            texts: snapshot_state.texts.clone(),
            fail_image_calls: 0,
            fail_text_calls: 0,
        };
        (outcome, observed)
    }

    #[test]
    fn copies_the_card_and_writes_the_same_card_to_a_png() {
        let dir = temp_dir("happy");
        let (outcome, observed) = share(&dir, 1_700_000_000, |_| {});

        assert_eq!(outcome.clipboard, ClipboardContent::Image);
        assert_eq!(
            observed.images.len(),
            1,
            "the card goes to the clipboard once"
        );

        let expected = dir.join("oc-stats-all-1700000000.png");
        assert_eq!(
            outcome.png,
            PngArtifact::Written(expected.display().to_string())
        );
        assert!(expected.exists(), "the card is on disk as well");

        // The file really holds the card that was copied, not an empty placeholder.
        let written = image::open(&expected).unwrap().to_rgba8();
        assert_eq!(
            (written.width(), written.height()),
            observed.images[0],
            "the PNG matches the image placed on the clipboard"
        );
        assert!(
            written.pixels().any(|pixel| pixel[3] > 0),
            "the PNG has visible pixels"
        );

        let message = outcome.message();
        assert!(
            message.contains("Copied share card to clipboard"),
            "{message}"
        );
        assert!(message.contains("oc-stats-all-1700000000.png"), "{message}");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn falls_back_to_the_text_summary_and_says_so() {
        let dir = temp_dir("fallback");
        let (outcome, observed) = share(&dir, 7, |state| state.fail_image_calls = 3);

        assert_eq!(
            outcome.clipboard,
            ClipboardContent::Text {
                reason: "image failed: the clipboard is held by another process".to_string()
            }
        );
        assert_eq!(
            observed.texts,
            vec!["oc-stats all time: 30 tokens".to_string()]
        );
        assert!(observed.images.is_empty());

        let message = outcome.message();
        assert!(message.contains("Copied text summary instead"), "{message}");
        assert!(message.contains("held by another process"), "{message}");
        assert!(
            message.contains("oc-stats-all-7.png"),
            "names the file: {message}"
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reports_the_error_and_still_writes_the_file_when_nothing_can_be_placed() {
        let dir = temp_dir("failed");
        let (outcome, _) = share(&dir, 9, |state| {
            state.fail_image_calls = 3;
            state.fail_text_calls = 3;
        });

        match &outcome.clipboard {
            ClipboardContent::Nothing { error } => {
                assert!(
                    error.contains("the clipboard is held by another process"),
                    "{error}"
                );
            }
            other => panic!("expected nothing to be placed, got {other:?}"),
        }
        assert!(
            matches!(outcome.png, PngArtifact::Written(_)),
            "the file is still written: {:?}",
            outcome.png
        );
        assert!(outcome.message().starts_with("Copy failed:"));
        assert!(dir.join("oc-stats-all-9.png").exists());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn retries_a_transient_failure_instead_of_giving_up() {
        let dir = temp_dir("retry");
        let (outcome, observed) = share(&dir, 11, |state| state.fail_image_calls = 1);

        assert_eq!(outcome.clipboard, ClipboardContent::Image);
        assert_eq!(observed.image_calls, 2, "the second attempt succeeded");
        assert_eq!(observed.images.len(), 1);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn two_copies_at_different_times_write_two_files() {
        let dir = temp_dir("twice");
        let (first, _) = share(&dir, 100, |_| {});
        let (second, _) = share(&dir, 200, |_| {});

        assert_ne!(first.png, second.png);
        assert!(dir.join("oc-stats-all-100.png").exists());
        assert!(dir.join("oc-stats-all-200.png").exists());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_message_names_what_reached_the_clipboard_and_the_file() {
        let cases = [
            (
                ShareOutcome {
                    clipboard: ClipboardContent::Image,
                    png: PngArtifact::Written("card.png".to_string()),
                },
                "Copied share card to clipboard | saved card.png",
            ),
            (
                ShareOutcome {
                    clipboard: ClipboardContent::Image,
                    png: PngArtifact::Failed("disk full".to_string()),
                },
                "Copied share card to clipboard | PNG write failed: disk full",
            ),
            (
                ShareOutcome {
                    clipboard: ClipboardContent::Text {
                        reason: "no image format".to_string(),
                    },
                    png: PngArtifact::Skipped,
                },
                "Copied text summary instead (no image format)",
            ),
            (
                ShareOutcome {
                    clipboard: ClipboardContent::Nothing {
                        error: "clipboard busy".to_string(),
                    },
                    png: PngArtifact::Written("card.png".to_string()),
                },
                "Copy failed: clipboard busy | saved card.png",
            ),
        ];

        for (outcome, expected) in cases {
            assert_eq!(outcome.message(), expected);
        }
    }
}
