use std::{
    collections::{BTreeMap, BTreeSet},
    hash::{BuildHasher, RandomState},
    ops::RangeInclusive,
};

use crate::{Res, sys};

pub const RANGE: RangeInclusive<u16> = 8000..=8999;

pub fn env_name(port: &str) -> String {
    format!("ADS_PORT_{}", port.to_ascii_uppercase().replace('-', "_"))
}

/// Starts at a random offset so concurrent stacks don't race for the same ports: a port is only
/// probed, not held, until its service binds it.
pub fn allocate(names: &BTreeSet<String>) -> Res<BTreeMap<String, u16>> {
    let len = u64::from(RANGE.end() - RANGE.start()) + 1;
    let offset = (RandomState::new().hash_one(std::process::id()) % len) as u16;
    allocate_with(names, RANGE, offset, sys::port_free)
}

fn allocate_with(
    names: &BTreeSet<String>,
    range: RangeInclusive<u16>,
    offset: u16,
    free: impl Fn(u16) -> bool,
) -> Res<BTreeMap<String, u16>> {
    let mut seen = BTreeMap::new();
    for n in names {
        if let Some(other) = seen.insert(env_name(n), n) {
            return Err(format!(
                "ports `{other}` and `{n}` both map to {}",
                env_name(n)
            ));
        }
    }
    let start = u32::from(*range.start());
    let len = u32::from(*range.end()) - start + 1;
    let mut tried = 0;
    let mut out = BTreeMap::new();
    for n in names {
        loop {
            if tried == len {
                return Err(format!(
                    "no free port for `{n}` in {}-{}",
                    range.start(),
                    range.end()
                ));
            }
            let p = (start + (u32::from(offset) + tried) % len) as u16;
            tried += 1;
            if free(p) {
                out.insert(n.clone(), p);
                break;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(n: &[&str]) -> BTreeSet<String> {
        n.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn sorted_sequential_skipping_busy() {
        let got =
            allocate_with(&names(&["web", "api", "db"]), 8000..=8010, 0, |p| p != 8001).unwrap();
        assert_eq!(
            got,
            BTreeMap::from([
                ("api".to_string(), 8000),
                ("db".to_string(), 8002),
                ("web".to_string(), 8003)
            ])
        );
    }

    #[test]
    fn offset_wraps_around() {
        let got = allocate_with(&names(&["a", "b", "c"]), 8000..=8009, 8, |p| p != 8009).unwrap();
        assert_eq!(
            got,
            BTreeMap::from([
                ("a".to_string(), 8008),
                ("b".to_string(), 8000),
                ("c".to_string(), 8001)
            ])
        );
        let e = allocate_with(&names(&["a", "b"]), 8000..=8001, 1, |p| p != 8001).unwrap_err();
        assert_eq!(e, "no free port for `b` in 8000-8001");
    }

    #[test]
    fn exhausted() {
        let e = allocate_with(&names(&["a", "b"]), 8000..=8001, 0, |p| p != 8001).unwrap_err();
        assert_eq!(e, "no free port for `b` in 8000-8001");
    }

    #[test]
    fn env_name_collision() {
        let e = allocate_with(&names(&["a-b", "a_b"]), RANGE, 0, |_| true).unwrap_err();
        assert!(e.contains("ADS_PORT_A_B"));
    }
}
