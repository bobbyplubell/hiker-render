//! Barycenter sort — a port of `dagre/lib/order/sort.ts`.
//!
//! Sorts the sortable entries (those that have a barycenter) by barycenter,
//! interleaving the fixed-position unsortable entries (no barycenter) at the
//! indices recorded in their `i`. `biasRight` breaks ties toward higher `i`.

use super::resolve_conflicts::ResolvedEntry;
use super::util;

/// Result of [`sort`] — TS `{vs, barycenter?, weight?}`.
#[derive(Clone, Debug, PartialEq)]
pub struct SortResult {
    pub vs: Vec<String>,
    pub barycenter: Option<f64>,
    pub weight: Option<f64>,
}

/// `sort(entries, biasRight)`.
pub fn sort(entries: Vec<ResolvedEntry>, bias_right: bool) -> SortResult {
    let parts = util::partition(entries, |e| e.barycenter.is_some());
    let mut sortable = parts.lhs;
    let mut unsortable = parts.rhs;
    // unsortable.sort((a, b) => b.i - a.i)  — descending by i.
    unsortable.sort_by(|a, b| b.i.cmp(&a.i));

    let mut vs: Vec<Vec<String>> = Vec::new();
    let mut sum = 0.0_f64;
    let mut weight = 0.0_f64;
    let mut vs_index = 0_usize;

    sortable.sort_by(|a, b| compare_with_bias(bias_right, a, b));

    vs_index = consume_unsortable(&mut vs, &mut unsortable, vs_index);

    for entry in &sortable {
        vs_index += entry.vs.len();
        vs.push(entry.vs.clone());
        sum += entry.barycenter.unwrap() * entry.weight.unwrap();
        weight += entry.weight.unwrap();
        vs_index = consume_unsortable(&mut vs, &mut unsortable, vs_index);
    }

    let flat: Vec<String> = vs.into_iter().flatten().collect();
    if weight != 0.0 {
        SortResult {
            vs: flat,
            barycenter: Some(sum / weight),
            weight: Some(weight),
        }
    } else {
        SortResult {
            vs: flat,
            barycenter: None,
            weight: None,
        }
    }
}

fn consume_unsortable(
    vs: &mut Vec<Vec<String>>,
    unsortable: &mut Vec<ResolvedEntry>,
    mut index: usize,
) -> usize {
    while let Some(last) = unsortable.last() {
        if last.i <= index {
            let last = unsortable.pop().unwrap();
            vs.push(last.vs);
            index += 1;
        } else {
            break;
        }
    }
    index
}

fn compare_with_bias(bias: bool, ev: &ResolvedEntry, ew: &ResolvedEntry) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let bv = ev.barycenter.unwrap();
    let bw = ew.barycenter.unwrap();
    if bv < bw {
        Ordering::Less
    } else if bv > bw {
        Ordering::Greater
    } else if !bias {
        ev.i.cmp(&ew.i)
    } else {
        ew.i.cmp(&ev.i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(vs: &[&str], i: usize, b: Option<f64>, w: Option<f64>) -> ResolvedEntry {
        ResolvedEntry {
            vs: vs.iter().map(|s| s.to_string()).collect(),
            i,
            barycenter: b,
            weight: w,
        }
    }

    fn vs(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn sorts_nodes_by_barycenter() {
        let input = vec![
            e(&["a"], 0, Some(2.0), Some(3.0)),
            e(&["b"], 1, Some(1.0), Some(2.0)),
        ];
        assert_eq!(
            sort(input, false),
            SortResult {
                vs: vs(&["b", "a"]),
                barycenter: Some((2.0 * 3.0 + 1.0 * 2.0) / (3.0 + 2.0)),
                weight: Some(3.0 + 2.0),
            }
        );
    }

    #[test]
    fn can_sort_super_nodes() {
        let input = vec![
            e(&["a", "c", "d"], 0, Some(2.0), Some(3.0)),
            e(&["b"], 1, Some(1.0), Some(2.0)),
        ];
        assert_eq!(
            sort(input, false),
            SortResult {
                vs: vs(&["b", "a", "c", "d"]),
                barycenter: Some((2.0 * 3.0 + 1.0 * 2.0) / (3.0 + 2.0)),
                weight: Some(3.0 + 2.0),
            }
        );
    }

    #[test]
    fn biases_to_left_by_default() {
        let input = vec![
            e(&["a"], 0, Some(1.0), Some(1.0)),
            e(&["b"], 1, Some(1.0), Some(1.0)),
        ];
        assert_eq!(
            sort(input, false),
            SortResult {
                vs: vs(&["a", "b"]),
                barycenter: Some(1.0),
                weight: Some(2.0),
            }
        );
    }

    #[test]
    fn biases_to_right_if_bias_right() {
        let input = vec![
            e(&["a"], 0, Some(1.0), Some(1.0)),
            e(&["b"], 1, Some(1.0), Some(1.0)),
        ];
        assert_eq!(
            sort(input, true),
            SortResult {
                vs: vs(&["b", "a"]),
                barycenter: Some(1.0),
                weight: Some(2.0),
            }
        );
    }

    #[test]
    fn can_sort_nodes_without_barycenter() {
        let input = vec![
            e(&["a"], 0, Some(2.0), Some(1.0)),
            e(&["b"], 1, Some(6.0), Some(1.0)),
            e(&["c"], 2, None, None),
            e(&["d"], 3, Some(3.0), Some(1.0)),
        ];
        assert_eq!(
            sort(input, false),
            SortResult {
                vs: vs(&["a", "d", "c", "b"]),
                barycenter: Some((2.0 + 6.0 + 3.0) / 3.0),
                weight: Some(3.0),
            }
        );
    }

    #[test]
    fn can_handle_no_barycenters_for_any_nodes() {
        let input = vec![
            e(&["a"], 0, None, None),
            e(&["b"], 3, None, None),
            e(&["c"], 2, None, None),
            e(&["d"], 1, None, None),
        ];
        assert_eq!(
            sort(input, false),
            SortResult {
                vs: vs(&["a", "d", "c", "b"]),
                barycenter: None,
                weight: None,
            }
        );
    }

    #[test]
    fn can_handle_barycenter_of_0() {
        let input = vec![
            e(&["a"], 0, Some(0.0), Some(1.0)),
            e(&["b"], 3, None, None),
            e(&["c"], 2, None, None),
            e(&["d"], 1, None, None),
        ];
        assert_eq!(
            sort(input, false),
            SortResult {
                vs: vs(&["a", "d", "c", "b"]),
                barycenter: Some(0.0),
                weight: Some(1.0),
            }
        );
    }
}
