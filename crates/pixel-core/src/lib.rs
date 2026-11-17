//! # pixel-core
//!
//! A pixel-perfect drawing engine built for AI agents:
//!
//! - **Fast**: 64×64 copy-on-write tiles, span rasterization, parallel painting and
//!   compositing with rayon, integer arithmetic for blending.
//! - **Exact**: pixel-center sampling, no antialiasing, deterministic output.
//! - **Cheap to verify**: per-tile diffs, zoomed views with rulers, exact pixel dumps.

pub mod canvas;
pub mod color;
pub mod doc;
pub mod history;
pub mod image;
pub mod paint;
pub mod path;
pub mod raster;
pub mod span;
pub mod text;
pub mod view;

pub use canvas::Raster;
pub use color::{BlendMode, Color};
pub use doc::{Diff, Document, Layer};
pub use history::{Author, History, Step};
pub use image::Image;
pub use paint::{Gradient, GradientShape, Paint};
pub use raster::{Cap, FillRule};
pub use span::{IRect, Span};

/// Largest accepted canvas side (16384 px = 268 M pixels when fully used).
pub const MAX_SIDE: u32 = 16384;
