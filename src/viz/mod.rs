//! Spectrum, channel meters, and the scope.
//!
//! The audio callback publishes a stereo window and four peak levels through
//! [`VizBus`]. Everything after that — the FFT, the log bars, the meter
//! ballistics, the braille canvas — is ordinary code the UI calls on its own
//! thread. `--render` does not draw any of this; the same functions are what
//! the tests call.

mod agc;
mod ballistics;
mod bands;
mod bus;
mod column;
mod fft;
mod scope;
mod state;

pub use agc::{AgcParams, AutoGain};
pub use ballistics::{smooth_spectrum, Ballistics, Meter};
pub use bands::{
    band_levels, db_unit, log_bands, spectrum_bars, spectrum_magnitudes, tilt_gain, Band, DB_FLOOR,
    F_MAX_HZ, F_MIN_HZ, TILT_DB_PER_OCTAVE,
};
pub(crate) use bus::VizAccum;
pub use bus::{VizBus, VizSnapshot};
pub use column::{spectrum_column, ColumnCell, ColumnInk, PEAK_CAP};
pub use fft::{apply_hann, magnitudes, windowed_magnitudes};
pub use scope::{draw_scope, ScopeCanvas, INK_CHANNEL, INK_GUIDE, INK_SCOPE};
pub use state::{channel_unit, meter_curve, VizState, BARS, METER_FLOOR_DB};

/// Samples in one analysis window. Also the FFT length.
pub const WINDOW: usize = 512;

/// Which visualization is up.
///
/// [`Self::Panel`] is the startup view. F5 still walks spectrum, then scope,
/// then off, because [`Self::cycle`] is unchanged and the tracker begins on
/// the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VizMode {
    /// The pattern editor, with no analyzer.
    Off,
    /// Spectrum and four channel meters under the pattern.
    #[default]
    Panel,
    /// Full-screen vectorscope.
    Scope,
}

impl VizMode {
    /// Off, then the panel, then the scope, then off again.
    pub fn cycle(self) -> Self {
        match self {
            Self::Off => Self::Panel,
            Self::Panel => Self::Scope,
            Self::Scope => Self::Off,
        }
    }

    /// Short name for the status line and `default_view`.
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Panel => "spectrum",
            Self::Scope => "scope",
        }
    }

    /// `spectrum`, `scope`, or `off`. Anything else is `None`.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "spectrum" | "panel" => Some(Self::Panel),
            "scope" => Some(Self::Scope),
            "off" | "hidden" => Some(Self::Off),
            _ => None,
        }
    }
}
