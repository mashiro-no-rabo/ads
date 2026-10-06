use std::{
    collections::{BTreeMap, BTreeSet},
    ops::RangeInclusive,
};

use crate::{Res, sys};

pub const RANGE: RangeInclusive<u16> = 8000..=8999;

pub fn env_name(port: &str) -> String {
    format!("ADS_PORT_{}", port.to_ascii_uppercase().replace('-', "_"))
}

pub fn allocate(names: &BTreeSet<String>) -> Res<BTreeMap<String, u16>> {
    allocate_with(names, RANGE, sys::port_free)
}

fn allocate_with(
    names: &BTreeSet<String>,
    range: RangeInclusive<u16>,
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
    let mut next = u32::from(*range.start());
    let end = u32::from(*range.end());
    let mut out = BTreeMap::new();
    for n in names {
        loop {
            if next > end {
                return Err(format!(
                    "no free port for `{n}` in {}-{}",
                    range.start(),
                    range.end()
                ));
            }
            let p = next as u16;
            next += 1;
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
        let got = allocate_with(&names(&["web", "api", "db"]), 8000..=8010, |p| p != 8001).unwrap();
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
    fn exhausted() {
        let e = allocate_with(&names(&["a", "b"]), 8000..=8001, |p| p != 8001).unwrap_err();
        assert_eq!(e, "no free port for `b` in 8000-8001");
    }

    #[test]
    fn env_name_collision() {
        let e = allocate_with(&names(&["a-b", "a_b"]), RANGE, |_| true).unwrap_err();
        assert!(e.contains("ADS_PORT_A_B"));
    }
}
