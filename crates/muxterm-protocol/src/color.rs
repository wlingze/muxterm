//! Product-level RGB values shared by configuration and runtime protocols.

/// An sRGB color represented by its three 8-bit channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn to_u32(self) -> u32 {
        ((self.0 as u32) << 16) | ((self.1 as u32) << 8) | (self.2 as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_rgb_channels_without_alpha() {
        assert_eq!(Rgb(0x12, 0x34, 0x56).to_u32(), 0x0012_3456);
        assert_eq!(Rgb(0xff, 0x00, 0xaa).to_u32(), 0x00ff_00aa);
    }
}
