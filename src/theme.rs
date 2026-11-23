//! The app's dark, minimal look, shared by the popover and the settings
//! window so their panels, borders, radii and text match.

pub type Rgb = [u8; 3];

pub const WINDOW: Rgb = [24, 24, 24];
pub const PANEL: Rgb = [44, 44, 44];
pub const TEXT: Rgb = [240, 240, 240];
pub const SUBTLE: Rgb = [160, 160, 160];
pub const BORDER: (Rgb, f32) = ([255, 255, 255], 0.10);
pub const SEPARATOR: (Rgb, f32) = ([255, 255, 255], 0.10);
pub const CONTROL_BORDER: (Rgb, f32) = ([255, 255, 255], 0.16);
pub const HOVER: (Rgb, f32) = ([255, 255, 255], 0.08);

/// Panel corner radius (the popover, the settings card).
pub const RADIUS: f32 = 6.0;
/// Inputs and segmented controls.
pub const CONTROL_RADIUS: f32 = 5.0;
/// Hover highlights inside a panel.
pub const HOVER_RADIUS: f32 = 4.0;

/// Window outline (the panel color, a little lighter).
pub const WINDOW_BORDER: Rgb = [66, 66, 66];
