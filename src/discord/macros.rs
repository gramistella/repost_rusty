/// This macro creates a crab with the given face.
#[macro_export]
macro_rules! crab {
    ($face:expr) => {
        format!("(V){{{}}}(V)", $face)
    };
}

/// This macro creates a big crab with the given face.
#[macro_export]
macro_rules! big_crab {
    ($face:expr) => {
        format!("(\\/){{{}}}(\\/)", $face)
    };
}

// export the macro
#[allow(unused_imports)]
pub use crab;

#[allow(unused_imports)]
pub use big_crab;
