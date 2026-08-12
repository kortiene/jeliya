//! The three target-shaped fixtures (#174 §6): browser, desktop, and Android.
//!
//! A [`Shape`] carries the **structural facts** each target presents — preference
//! durability, whether window actions and a private directory exist, whether a
//! backup-excluded protected directory exists, and the default picked-source
//! object kind — so shared components compile and behave against each target
//! without a device and without a `cfg` fork.

use crate::error::Availability;
use crate::files::FileObjectKind;
use crate::storage::Durability;

/// Which target a fake models.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    /// An ordinary browser tab: session-scoped storage; no window actions; no
    /// private directory; browser-blob sources; server-side staging.
    Browser,
    /// Packaged desktop: persistent storage; window actions; a private
    /// directory (app-support); native-path sources; copy-then-delete staging.
    Desktop,
    /// Android: persistent storage; a protected, backup-excluded private
    /// directory; no window actions; `content://` sources streamed through
    /// bounded staging.
    Android,
}

impl Shape {
    /// Preference-store durability for this shape.
    pub fn durability(self) -> Durability {
        match self {
            Shape::Browser => Durability::SessionScoped,
            Shape::Desktop | Shape::Android => Durability::Persistent,
        }
    }

    /// Whether window actions are available.
    pub fn window_availability(self) -> Availability {
        match self {
            Shape::Desktop => Availability::Available,
            Shape::Browser | Shape::Android => Availability::Unavailable,
        }
    }

    /// Whether the protected/private directory capability is available.
    pub fn private_directory_availability(self) -> Availability {
        match self {
            Shape::Desktop | Shape::Android => Availability::Available,
            Shape::Browser => Availability::Unavailable,
        }
    }

    /// Whether the private directory is excluded from OS backups. Android's
    /// `<noBackupFilesDir>` is; a desktop app-support directory is not.
    pub fn private_directory_backup_excluded(self) -> bool {
        matches!(self, Shape::Android)
    }

    /// The default picked-source object kind for this shape.
    pub fn source_kind(self) -> FileObjectKind {
        match self {
            Shape::Browser => FileObjectKind::BrowserBlob,
            Shape::Desktop => FileObjectKind::NativePath,
            Shape::Android => FileObjectKind::ContentUri,
        }
    }
}
