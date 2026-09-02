//! Shelf/next-fit packing for one square atlas texture.

use crookui_core::geometry::{RectF, vec2f};

use super::{AllocatedRegion, AllocationError, PixelRect};

/// Gap left to the right of each item, in pixels.
///
/// The glyph sampler filters linearly, so without a gap it would blend a
/// neighbouring glyph in at the edges of every quad.
const HORIZONTAL_PADDING: i32 = 1;

/// Gap left below each row of items, in pixels.
const VERTICAL_PADDING: i32 = 1;

/// Decides where items go in one atlas texture.
///
/// Items are packed left to right along the current row — the "shelf" — until
/// one does not fit, at which point the shelf closes at the height of its
/// tallest item and a new one opens below it:
///
/// ```text
///                           (width, height)
///   ┌─────┬─────┬─────┬─────┬─────┐
///   │ 10  │     │     │     │     │ <- room above a short row is wasted; the
///   │     │     │     │     │     │    next shelf starts below the tallest item
///   ├─────┼─────┼─────┼─────┼─────┤
///   │ 5   │ 6   │ 7   │ 8   │ 9   │
///   │     │     │     │     │     │
///   ├─────┼─────┼─────┼─────┴─────┤
///   │ 1   │ 2   │ 3   │ 4         │
///   │     │     │     │           │ <- a row closes when the next item does
///   └─────┴─────┴─────┴───────────┘    not fit in it
/// (0, 0)  x->
/// ```
///
/// The wasted space above short items is the price of an allocator with no
/// bookkeeping beyond three integers. Glyphs of one font at one size are all
/// about as tall as each other, so in practice very little is lost.
#[derive(Debug)]
pub struct Allocator {
    width: i32,
    height: i32,

    /// The leftmost free pixel in the current row.
    row_extent: i32,

    /// The top of the current row.
    row_baseline: i32,

    /// The tallest item placed in the current row, which is how far down the
    /// next row starts.
    row_tallest: i32,
}

impl Allocator {
    /// An empty allocator for a `size` × `size` atlas.
    pub fn new(size: u32) -> Self {
        Self {
            width: size as i32,
            height: size as i32,
            row_extent: 0,
            row_baseline: 0,
            row_tallest: 0,
        }
    }

    /// Finds room for an item of `size` pixels.
    pub fn insert(&mut self, size: (u32, u32)) -> Result<AllocatedRegion, AllocationError> {
        let (width, height) = (size.0 as i32, size.1 as i32);
        if width > self.width || height > self.height {
            return Err(AllocationError::ItemTooLarge);
        }

        if !self.fits_in_row(width, height) {
            self.advance_row()?;
        }

        if !self.fits_in_row(width, height) {
            return Err(AllocationError::Full);
        }

        Ok(self.place(width, height))
    }

    fn place(&mut self, width: i32, height: i32) -> AllocatedRegion {
        let x = self.row_extent;
        let y = self.row_baseline;

        self.row_extent = x + width + HORIZONTAL_PADDING;
        self.row_tallest = self.row_tallest.max(height);

        AllocatedRegion {
            uv_region: RectF::new(
                vec2f(x as f32 / self.width as f32, y as f32 / self.height as f32),
                vec2f(
                    width as f32 / self.width as f32,
                    height as f32 / self.height as f32,
                ),
            ),
            pixel_region: PixelRect {
                x: x as u32,
                y: y as u32,
                width: width as u32,
                height: height as u32,
            },
        }
    }

    fn fits_in_row(&self, width: i32, height: i32) -> bool {
        self.row_extent + width <= self.width && height < self.height - self.row_baseline
    }

    fn advance_row(&mut self) -> Result<(), AllocationError> {
        let next_baseline = self.row_baseline + self.row_tallest + VERTICAL_PADDING;
        if next_baseline >= self.height {
            return Err(AllocationError::Full);
        }

        self.row_baseline = next_baseline;
        self.row_extent = 0;
        self.row_tallest = 0;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_pack_left_to_right_with_a_gap_between_them() {
        let mut allocator = Allocator::new(64);

        let first = allocator.insert((10, 8)).unwrap().pixel_region;
        let second = allocator.insert((10, 8)).unwrap().pixel_region;

        assert_eq!((first.x, first.y), (0, 0));
        assert_eq!((second.x, second.y), (11, 0));
    }

    #[test]
    fn a_row_closes_below_its_tallest_item() {
        let mut allocator = Allocator::new(32);

        allocator.insert((20, 12)).unwrap();
        allocator.insert((8, 4)).unwrap();
        // 20 + 1 + 8 + 1 = 30 used, so a 10-wide item opens a new row.
        let wrapped = allocator.insert((10, 4)).unwrap().pixel_region;

        assert_eq!((wrapped.x, wrapped.y), (0, 13));
    }

    #[test]
    fn uv_coordinates_are_the_pixel_region_over_the_atlas_size() {
        let mut allocator = Allocator::new(100);

        let region = allocator.insert((25, 50)).unwrap();

        assert_eq!(region.uv_region.origin(), vec2f(0., 0.));
        assert_eq!(region.uv_region.size(), vec2f(0.25, 0.5));
    }

    #[test]
    fn a_full_atlas_reports_full_and_an_oversized_item_reports_too_large() {
        let mut allocator = Allocator::new(16);

        assert_eq!(
            allocator.insert((20, 4)).err(),
            Some(AllocationError::ItemTooLarge)
        );

        allocator.insert((16, 14)).unwrap();
        assert_eq!(
            allocator.insert((16, 14)).err(),
            Some(AllocationError::Full)
        );
    }
}
