//! The guild's ASCII art, after the crest on the spec's cover: a shield with a
//! four-point star, a striped fin rising out of the waves.
//!
//! Plain ASCII only, so it looks the same in the classic Windows console. To see
//! a change without running the app: `cargo test preview -- --ignored --nocapture`.

/// The full crest, for the start-up splash.
pub const CREST: &[&str] = &[
    r"  ___________________  ",
    r" |                   | ",
    r" |    .         _.-| | ",
    r" |   -+-     _.' / | | ",
    r" |    '   _.'  / / | | ",
    r" |      .'   / / / | | ",
    r" |   ~^~~^~~^~~^~~^~ | ",
    r" |    ~^~~^~~^~~^~   | ",
    r"  \                 /  ",
    r"   '.             .'   ",
    r"     '-.       .-'     ",
    r"        '-._.-'        ",
];

/// A small crest for the dashboard header, on tall terminals.
pub const CREST_SMALL: &[&str] = &[
    r" .-------. ",
    r" | + _.-|| ",
    r" |~^~^~^~| ",
    r"  \     /  ",
    r"   '-.-'   ",
];

/// The guild's name, spaced out as on the spec's cover.
pub const NAME: &str = "M U R L O C   D E F E N C E   L E A G U E";

/// Under the name on the splash.
pub const MOTTO: &str = "mrglglglgl!";
