//! Rolling over to a new atlas texture when the current one fills up.

use super::{AllocatedRegion, AllocationError, Allocator};

/// Which atlas texture a region lives in.
///
/// Ordinal rather than opaque: the renderer indexes a `Vec` of textures with
/// it, and drawing groups glyphs by it so the number of bind-group switches in
/// a layer is the number of atlases, not the number of glyphs.
#[derive(Copy, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TextureId(usize);

impl TextureId {
    /// The id of the first atlas a fresh [`Manager`] allocates into.
    pub fn initial() -> Self {
        Self(0)
    }

    /// The id after this one.
    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }

    /// This id as an index into the renderer's texture list.
    pub fn as_index(self) -> usize {
        self.0
    }
}

/// Where an item ended up: which texture, and where inside it.
#[derive(Copy, Clone, Debug)]
pub struct TextureOffset {
    /// The atlas that took the item.
    pub texture_id: TextureId,
    /// The region within that atlas.
    pub allocated_region: AllocatedRegion,
}

/// Allocates into a series of same-sized atlas textures.
pub struct Manager {
    current_allocator: Allocator,
    current_texture_id: TextureId,
    atlas_size: u32,
}

impl Manager {
    /// A manager whose atlases are `atlas_size` × `atlas_size` pixels.
    pub fn new(atlas_size: u32) -> Self {
        Self {
            current_allocator: Allocator::new(atlas_size),
            current_texture_id: TextureId::initial(),
            atlas_size,
        }
    }

    /// Finds room for an item of `size` pixels, opening a new atlas if the
    /// current one is full.
    ///
    /// Fails only when the item is larger than a whole atlas, which no amount
    /// of new textures would fix.
    pub fn insert(&mut self, size: (u32, u32)) -> Result<TextureOffset, AllocationError> {
        match self.current_allocator.insert(size) {
            Ok(allocated_region) => Ok(TextureOffset {
                texture_id: self.current_texture_id,
                allocated_region,
            }),
            Err(AllocationError::Full) => {
                self.current_texture_id = self.current_texture_id.next();
                self.current_allocator = Allocator::new(self.atlas_size);
                self.current_allocator
                    .insert(size)
                    .map(|allocated_region| TextureOffset {
                        texture_id: self.current_texture_id,
                        allocated_region,
                    })
            }
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_atlas_rolls_over_to_the_next_texture() {
        let mut manager = Manager::new(16);

        let first = manager.insert((16, 14)).unwrap();
        let second = manager.insert((16, 14)).unwrap();

        assert_eq!(first.texture_id, TextureId::initial());
        assert_eq!(second.texture_id, TextureId::initial().next());
        assert_eq!(second.allocated_region.pixel_region.y, 0);
    }

    #[test]
    fn an_item_larger_than_an_atlas_is_never_placed() {
        let mut manager = Manager::new(16);

        assert_eq!(
            manager.insert((32, 4)).err(),
            Some(AllocationError::ItemTooLarge)
        );
    }
}
