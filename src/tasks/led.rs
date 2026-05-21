//! LED task — consumes `LedCommand { Pattern, Colour }` and drives the
//! three LEDC channels accordingly.
//!
//! Stack: ~2 KB target.
//!
//! TODO(phase-1): basic colours (red, amber, green, blue, off) and three
//! patterns (solid, slow blink, fast blink).
