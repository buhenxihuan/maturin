use super::PageTableRoot;

pub type IdentType = super::PTEFlags;
pub type ArgsType = (PageTableRoot,);

/// 4 KB
pub const LOWER_LIMIT: usize = 0x1000;
/// 4 GB
pub const UPPER_LIMIT: usize = 0x1_0000_0000;
