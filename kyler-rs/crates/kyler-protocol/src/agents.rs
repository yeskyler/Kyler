//! Stable agent display identities and delegation ceilings.
pub const PRIMARY_AGENT_NAME: &str = "kyler";
pub const MAX_CONCURRENT_SUBAGENTS: usize = 32;
pub const MAX_SUBAGENTS_PER_EXECUTION: usize = 64;

pub const SUBAGENT_NAMES: [&str; 32] = [
    "atlas", "nova", "orion", "lyra", "vega", "sirius", "altair", "rigel", "deneb", "mira", "leo",
    "aries", "cygnus", "draco", "phoenix", "hydra", "perseus", "auriga", "vela", "carina",
    "corvus", "pavo", "tucana", "grus", "dorado", "fornax", "sculptor", "pictor", "indus", "lupus",
    "lynx", "volans",
];

/// Zero-based creation ordinal within one parent execution; completed names are not reused.
pub fn subagent_name(ordinal: usize) -> String {
    let name = SUBAGENT_NAMES[ordinal % SUBAGENT_NAMES.len()];
    let generation = ordinal / SUBAGENT_NAMES.len();
    if generation == 0 {
        name.into()
    } else {
        format!("{name}-{}", generation + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_are_unique_and_never_use_the_primary_identity() {
        let names: std::collections::HashSet<_> = (0..MAX_SUBAGENTS_PER_EXECUTION)
            .map(subagent_name)
            .collect();
        assert_eq!(names.len(), 64);
        assert!(!names.contains(PRIMARY_AGENT_NAME));
        assert_eq!(subagent_name(0), "atlas");
        assert_eq!(subagent_name(32), "atlas-2");
        assert_eq!(subagent_name(63), "volans-2");
    }
}
