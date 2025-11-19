pub mod bind_group_layout;
pub mod bind_table_layout;
pub mod font;
pub mod geometry;
pub mod imagery;
pub mod primitives;
pub mod render_pass;
pub mod shader;

pub use bind_group_layout::*;
pub use bind_table_layout::*;
pub use font::*;
pub use geometry::*;
pub use imagery::*;
pub use render_pass::*;
pub use shader::*;

pub type DatabaseEntry = &'static str;

/// Returns a `'static` database entry string by leaking the provided value.
pub fn leak_database_entry(entry: &str) -> DatabaseEntry {
    Box::leak(entry.to_string().into_boxed_str())
}
