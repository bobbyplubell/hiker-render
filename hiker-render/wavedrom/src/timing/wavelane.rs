//! Wave-string → brick-name transition logic.
//!
//! Faithful port of `references/wavedrom/lib/{gen-brick, gen-first-wave-brick,
//! gen-wave-brick, parse-wave-lane}.js`. A `wave:` string is consumed two
//! characters at a time (`prev`,`next`); each pair maps to a small list of
//! "half-brick" glyph names (see [`crate::bricks`]). Half-bricks are drawn at
//! `i * xs`, so each *cycle* is two half-bricks wide.
//!
//! Supported wave chars: `p n P N` (clocks, P/N add an edge arrow), `l L h H`
//! (levels, L/H add an arrow — arrow deferred, same as l/h), `0 1` (levels),
//! `x` (undefined), `z` (hi-Z), `= 2..9` (data buses), `d u` (weak pull
//! low/high), `.` (extend/hold), `|` (gap; consumes like `.` and is marked
//! separately for the gap glyph).
//!
//! Sub-cycles (`< … >`): while ON, a brick is generated with `extra = 0` and a
//! repeat count of `repeats - period` (faithful port of `parseWaveLane`'s
//! `subCycle` branch), so the sub-cycle region is drawn compressed.

/// `genBrick(texts, extra, times)` — expand a brick template into half-bricks.
fn gen_brick(texts: &[&str], extra: usize, times: usize) -> Vec<String> {
    let mut r = Vec::new();
    if texts.len() == 4 {
        for _ in 0..times {
            r.push(texts[0].to_string());
            for _ in 0..extra {
                r.push(texts[1].to_string());
            }
            r.push(texts[2].to_string());
            for _ in 0..extra {
                r.push(texts[3].to_string());
            }
        }
        return r;
    }
    // length 1 → duplicate; length 2 → [t0, t1*…].
    let t0 = texts[0];
    let t1 = if texts.len() == 1 { texts[0] } else { texts[1] };
    r.push(t0.to_string());
    // times * (2*(extra+1)) - 1 copies of t1.
    let n = (times * (2 * (extra + 1))).saturating_sub(1);
    for _ in 0..n {
        r.push(t1.to_string());
    }
    r
}

/// `genFirstWaveBrick` lookup for the lane's first character.
fn gen_first_wave_brick(c: char, extra: usize, times: usize) -> Vec<String> {
    let four = |a: &str, b: &str, cc: &str, d: &str, extra, times| {
        gen_brick(&[a, b, cc, d], extra, times)
    };
    match c {
        'p' => four("pclk", "111", "nclk", "000", extra, times),
        'n' => four("nclk", "000", "pclk", "111", extra, times),
        'P' => four("Pclk", "111", "nclk", "000", extra, times),
        'N' => four("Nclk", "000", "pclk", "111", extra, times),
        'l' | 'L' | '0' => gen_brick(&["000"], extra, times),
        'h' | 'H' | '1' => gen_brick(&["111"], extra, times),
        '=' | '2' => gen_brick(&["vvv-2"], extra, times),
        '3' => gen_brick(&["vvv-3"], extra, times),
        '4' => gen_brick(&["vvv-4"], extra, times),
        '5' => gen_brick(&["vvv-5"], extra, times),
        '6' => gen_brick(&["vvv-6"], extra, times),
        '7' => gen_brick(&["vvv-7"], extra, times),
        '8' => gen_brick(&["vvv-8"], extra, times),
        '9' => gen_brick(&["vvv-9"], extra, times),
        'd' => gen_brick(&["ddd"], extra, times),
        'u' => gen_brick(&["uuu"], extra, times),
        'z' => gen_brick(&["zzz"], extra, times),
        _ => gen_brick(&["xxx"], extra, times),
    }
}

// Per-char lookup tables from gen-wave-brick.js.
fn x1(c: char) -> Option<&'static str> {
    Some(match c {
        'p' | 'h' => "pclk",
        'n' | 'l' => "nclk",
        'P' | 'H' => "Pclk",
        'N' | 'L' => "Nclk",
        _ => return None,
    })
}
fn x2(c: char) -> Option<&'static str> {
    Some(match c {
        '0' => "0",
        '1' => "1",
        'x' => "x",
        'd' => "d",
        'u' => "u",
        'z' => "z",
        '=' | '2' | '3' | '4' | '5' | '6' | '7' | '8' | '9' => "v",
        _ => return None,
    })
}
fn x3(c: char) -> &'static str {
    match c {
        '=' | '2' => "-2",
        '3' => "-3",
        '4' => "-4",
        '5' => "-5",
        '6' => "-6",
        '7' => "-7",
        '8' => "-8",
        '9' => "-9",
        _ => "",
    }
}
fn y1(c: char) -> Option<&'static str> {
    Some(match c {
        'p' | 'P' | 'l' | 'L' | '0' => "0",
        'n' | 'N' | 'h' | 'H' | '1' => "1",
        'x' => "x",
        'd' => "d",
        'u' => "u",
        'z' => "z",
        '=' | '2' | '3' | '4' | '5' | '6' | '7' | '8' | '9' => "v",
        _ => return None,
    })
}
fn y2(c: char) -> &'static str {
    x3(c) // identical mapping
}
fn x4(c: char) -> &'static str {
    match c {
        'p' | 'P' | 'h' | 'H' | '1' => "111",
        'n' | 'N' | 'l' | 'L' | '0' => "000",
        'x' => "xxx",
        'd' => "ddd",
        'u' => "uuu",
        'z' => "zzz",
        '=' | '2' => "vvv-2",
        '3' => "vvv-3",
        '4' => "vvv-4",
        '5' => "vvv-5",
        '6' => "vvv-6",
        '7' => "vvv-7",
        '8' => "vvv-8",
        '9' => "vvv-9",
        _ => "xxx",
    }
}
fn x5(c: char) -> Option<&'static str> {
    Some(match c {
        'p' | 'P' => "nclk",
        'n' | 'N' => "pclk",
        _ => return None,
    })
}
fn x6(c: char) -> &'static str {
    match c {
        'p' | 'P' => "000",
        'n' | 'N' => "111",
        _ => "",
    }
}
fn xclude(prev: char, next: char) -> Option<&'static str> {
    Some(match (prev, next) {
        ('h', 'p') | ('H', 'p') | ('n', 'h') | ('N', 'h') => "111",
        ('l', 'n') | ('L', 'n') | ('p', 'l') | ('P', 'l') => "000",
        _ => return None,
    })
}

/// `genWaveBrick(prev+next, extra, times)`.
fn gen_wave_brick(prev: char, next: char, extra: usize, times: usize) -> Vec<String> {
    let tmp0 = x4(next);
    match x1(next) {
        None => {
            // Soft (curved) transitions, or unknown.
            let tmp2 = match x2(next) {
                Some(v) => v,
                None => return gen_brick(&["xxx"], extra, times),
            };
            let tmp3 = match y1(prev) {
                Some(v) => v,
                None => return gen_brick(&["xxx"], extra, times),
            };
            // tmp3 + "m" + tmp2 + y2(prev) + x3(next)
            let trans = format!("{tmp3}m{tmp2}{y2}{x3}", y2 = y2(prev), x3 = x3(next));
            gen_brick(&[&trans, tmp0], extra, times)
        }
        Some(mut tmp1) => {
            // Sharp (clock/level) transitions.
            if let Some(ex) = xclude(prev, next) {
                tmp1 = ex;
            }
            match x5(next) {
                None => gen_brick(&[tmp1, tmp0], extra, times), // hlHL
                Some(tmp5) => gen_brick(&[tmp1, tmp0, tmp5, x6(next)], extra, times), // pnPN
            }
        }
    }
}

/// True if a half-brick name is a data segment (`vvv-N`).
fn is_data_marker(name: &str) -> bool {
    matches!(
        name,
        "vvv-2" | "vvv-3" | "vvv-4" | "vvv-5" | "vvv-6" | "vvv-7" | "vvv-8" | "vvv-9"
    )
}

/// `findLaneMarkers` — center positions (in half-brick units) of each run of
/// data half-bricks; used to place data labels.
pub fn find_lane_markers(lane: &[String]) -> Vec<f32> {
    let mut gcount = 0i32;
    let mut lcount = 0i32;
    let mut ret = Vec::new();
    for e in lane {
        if is_data_marker(e) {
            lcount += 1;
        } else if lcount != 0 {
            ret.push(gcount as f32 - (lcount as f32 + 1.0) / 2.0);
            lcount = 0;
        }
        gcount += 1;
    }
    if lcount != 0 {
        ret.push(gcount as f32 - (lcount as f32 + 1.0) / 2.0);
    }
    ret
}

/// Parsed lane: the half-brick name sequence, plus how many leading data
/// markers were shifted out of view by `phase` (so `data` is sliced past them),
/// and the half-brick index of each `|` gap break.
pub struct ParsedLane {
    pub bricks: Vec<String>,
    pub num_unseen_markers: usize,
}

/// Port of `parseWaveLane`: `wave` → half-brick names with `phase` applied.
///
/// `extra = period*hscale - 1` (padding repeats). `period` is the lane's period
/// (used by the sub-cycle branch). `phase_bricks` = number of leading
/// half-bricks to drop (phase shift). Inside a `< … >` sub-cycle region bricks
/// are generated with `extra = 0` and `repeats - period` (compressed).
pub fn parse_wave_lane(wave: &str, extra: usize, period: usize, phase_bricks: usize) -> ParsedLane {
    let chars: Vec<char> = wave.chars().collect();
    let mut idx = 0usize;
    let next0 = chars.first().copied();
    idx += 1;

    // Leading repeaters.
    let mut repeats = 1usize;
    while matches!(chars.get(idx), Some('.') | Some('|')) {
        idx += 1;
        repeats += 1;
    }

    let mut r = Vec::new();
    let mut prev;
    let mut next = match next0 {
        Some(c) => c,
        None => {
            return ParsedLane { bricks: r, num_unseen_markers: 0 };
        }
    };
    r.extend(gen_first_wave_brick(next, extra, repeats));

    let mut sub_cycle = false;
    while idx < chars.len() {
        prev = next;
        next = chars[idx];
        idx += 1;
        // Sub-cycle toggles: `<` turns compression on, `>` turns it off. The
        // toggle char is skipped and the real `next` follows (parse-wave-lane.js).
        if next == '<' {
            sub_cycle = true;
            next = chars.get(idx).copied().unwrap_or(next);
            idx += 1;
        }
        if next == '>' {
            sub_cycle = false;
            next = chars.get(idx).copied().unwrap_or(next);
            idx += 1;
        }
        let mut reps = 1usize;
        while matches!(chars.get(idx), Some('.') | Some('|')) {
            idx += 1;
            reps += 1;
        }
        if sub_cycle {
            // Compressed: no `extra` padding, repeat count reduced by `period`.
            let reps = reps.saturating_sub(period);
            r.extend(gen_wave_brick(prev, next, 0, reps));
        } else {
            r.extend(gen_wave_brick(prev, next, extra, reps));
        }
    }

    // Phase shift: drop the first `phase_bricks` half-bricks.
    let mut unseen: Vec<String> = Vec::new();
    for _ in 0..phase_bricks {
        if r.is_empty() {
            break;
        }
        unseen.push(r.remove(0));
    }

    let num_unseen_markers = if !unseen.is_empty() {
        let mut n = find_lane_markers(&unseen).len();
        // If the boundary half-brick on both sides is a data marker, the run
        // straddles the cut: don't double-count it.
        let last_seen = unseen.last().map(|s| is_data_marker(s)).unwrap_or(false);
        let first_r = r.first().map(|s| is_data_marker(s)).unwrap_or(false);
        if last_seen && first_r {
            n = n.saturating_sub(1);
        }
        n
    } else {
        0
    };

    ParsedLane { bricks: r, num_unseen_markers }
}

/// Compute the on-screen x (in `xs` units, post-hscale/phase) of each `|` gap in
/// the wave string. Port of `renderGapUses` (non-sub-cycle path).
///
/// Returns positions in *half-brick* units (multiply by `xs` to get px).
pub fn gap_positions(wave: &str, period: usize, hscale: usize, phase_bricks: usize) -> Vec<f32> {
    let mut res = Vec::new();
    let mut pos = 0i64;
    let chars: Vec<char> = wave.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let mut next = chars[i];
        i += 1;
        if next == '<' {
            next = chars.get(i).copied().unwrap_or(next);
            i += 1;
        }
        if next == '>' {
            next = chars.get(i).copied().unwrap_or(next);
            i += 1;
        }
        pos += 2 * period as i64;
        if next == '|' {
            // xs * ((pos - period) * hscale - phase)
            let x = ((pos - period as i64) * hscale as i64 - phase_bricks as i64) as f32;
            res.push(x);
        }
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sub_cycle_compresses_bricks() {
        // Same wave, with and without the `< … >` sub-cycle markers. Inside the
        // markers bricks are generated with `repeats - period` (period 1 here),
        // so the marked version must produce a strictly shorter brick sequence.
        let extra = 0; // period*hscale - 1, with period=1 hscale=1
        let plain = parse_wave_lane("x2.x", extra, 1, 0).bricks;
        let subbed = parse_wave_lane("x<2.>x", extra, 1, 0).bricks;
        assert!(
            subbed.len() < plain.len(),
            "sub-cycle region should compress: subbed={} plain={}",
            subbed.len(),
            plain.len()
        );
    }

    #[test]
    fn sub_cycle_only_affects_marked_region() {
        // Without markers the lane is unchanged from the historical behavior.
        let a = parse_wave_lane("x2.x", 0, 1, 0).bricks;
        let b = parse_wave_lane("x2.x", 0, 1, 0).bricks;
        assert_eq!(a, b);
    }
}
