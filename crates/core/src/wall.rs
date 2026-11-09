//! The shared wall: drawings on a room's backdrop, on a fixed grid, merged cell by cell so that every device ends up with the
//! same picture whatever order the batches reach it in. It rides on the live channel (`wall.d` strokes, `wall.s` snapshot
//! chunks), so it is end-to-end encrypted like everything else, and the relay learns nothing new.
//!
//! Rules every receiver enforces on its own (nothing here trusts the sender beyond "is a member of this room"):
//!  * every coordinate, glyph, colour, clock and batch size is checked, and a batch with any bad part is dropped whole;
//!  * a cell is won by the higher logical clock, ties broken by account, so merging is commutative and idempotent;
//!  * the wall has a generation, raised by the owner in the signed style document (that is "clear"): anything of another
//!    generation is ignored, so a device that was offline cannot bring a cleared wall back;
//!  * when the owner has locked the room only the owner's strokes count.
use crate::client::*;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const WALL_COLS: u32 = 120;
pub const WALL_ROWS: u32 = 54;
pub const WALL_CELLS: u32 = WALL_COLS * WALL_ROWS;
/// The glyphs a cell may hold, by index; index 0 is "erased". Only ever appended to.
pub const WALL_GLYPHS: [char; 12] = [' ', '#', '*', '+', '.', 'o', '=', '~', '/', '|', '-', 'x'];
/// Most cells in one stroke batch.
pub const WALL_BATCH: usize = 200;
/// A snapshot is at most this many events, each within the live limit.
pub const WALL_SNAP_CHUNKS: usize = 8;
const SNAP_BYTES: usize = 3700;
/// Walls kept on one device (the least recently touched go first).
pub const WALL_MAX_WALLS: usize = 16;
/// Changes reach the saved state at most this often.
pub const WALL_FLUSH_SECS: u64 = 20;
const MAX_CLOCK: u64 = 1 << 40;
/// How far past the highest clock this device has seen one batch may claim to be. An honest device moves up by one per stroke, so this only stops
/// a forged clock from jumping to the ceiling, where nobody could draw any more until the owner cleared the wall.
const MAX_CLOCK_STEP: u64 = 1 << 20;
const MAX_ACCTS: usize = 1024;

/// glyph index, colour (0 = the room's main colour, 1 = its highlight), logical clock, index into `Wall::accts`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell(pub u8, pub u8, pub u64, pub u16);

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Wall {
    pub gen: u64,
    /// The highest clock seen: our next stroke is one above it.
    pub clock: u64,
    pub accts: Vec<String>,
    /// Keyed by `y * WALL_COLS + x`. An erased cell is kept (glyph 0) so that an older stroke arriving late cannot bring it back.
    pub cells: BTreeMap<u16, Cell>,
    pub touched: u64,
}

fn is_acct(s: &str) -> bool { s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit()) }

impl Wall {
    fn idx(&mut self, who: &str) -> Option<u16> {
        if let Some(i) = self.accts.iter().position(|a| a == who) { return Some(i as u16); }
        if self.accts.len() >= MAX_ACCTS { return None; }
        self.accts.push(who.to_string());
        Some((self.accts.len() - 1) as u16)
    }
    /// Take one cell if it beats what is there. Returns whether the wall changed.
    pub fn put(&mut self, pos: u32, glyph: u8, col: u8, clock: u64, who: &str) -> bool {
        if pos >= WALL_CELLS || glyph as usize >= WALL_GLYPHS.len() || col > 1 || clock == 0 || clock > MAX_CLOCK || !is_acct(who) { return false; }
        let Some(ai) = self.idx(who) else { return false };
        if let Some(&Cell(_, _, k, a)) = self.cells.get(&(pos as u16)) {
            if (k, self.accts[a as usize].as_str()) >= (clock, who) { self.clock = self.clock.max(clock); return false; }
        }
        self.cells.insert(pos as u16, Cell(glyph, col, clock, ai));
        self.clock = self.clock.max(clock);
        true
    }
    /// Start over at generation `gen` if that is newer than ours.
    pub fn sync_gen(&mut self, gen: u64) { if gen > self.gen { *self = Wall { gen, touched: self.touched, ..Default::default() }; } }
    pub fn author(&self, c: &Cell) -> &str { self.accts.get(c.3 as usize).map(|s| s.as_str()).unwrap_or("") }
    /// Snapshot chunks, newest cells first, at most `WALL_SNAP_CHUNKS` events of at most 4096 bytes each. A wall with more cells than
    /// that holds loses its oldest ones from the snapshot (they stay on the devices that drew them).
    pub fn snapshot(&self) -> Vec<String> {
        type Item<'a> = (u16, u8, u8, u64, &'a str);
        let mut order: Vec<(&u16, &Cell)> = self.cells.iter().collect();
        order.sort_by(|a, b| b.1.2.cmp(&a.1.2).then(a.0.cmp(b.0)));
        let mut chunks: Vec<Vec<Item>> = vec![];
        let (mut cur, mut accts, mut used): (Vec<Item>, Vec<&str>, usize) = (vec![], vec![], 60);
        for (pos, c) in order {
            let who = self.author(c);
            let mut extra = if accts.contains(&who) { 0 } else { 68 };
            if (used + extra + 28 > SNAP_BYTES || (extra > 0 && accts.len() >= 16)) && !cur.is_empty() {
                chunks.push(std::mem::take(&mut cur)); accts.clear(); used = 60; extra = 68;
                if chunks.len() == WALL_SNAP_CHUNKS { break; }
            }
            if extra > 0 { accts.push(who); }
            used += extra + 28;
            cur.push((*pos, c.0, c.1, c.2, who));
        }
        if !cur.is_empty() && chunks.len() < WALL_SNAP_CHUNKS { chunks.push(cur); }
        let n = chunks.len();
        chunks.into_iter().enumerate().map(|(i, items)| {
            let mut accts: Vec<&str> = vec![];
            let cells: Vec<String> = items.iter().map(|&(p, g, c, k, who)| {
                let ai = accts.iter().position(|a| *a == who).unwrap_or_else(|| { accts.push(who); accts.len() - 1 });
                format!("[{},{},{},{}]", p, g as u32 * 2 + c as u32, k, ai)
            }).collect();
            format!("{{\"g\":{},\"i\":{},\"n\":{},\"a\":[{}],\"c\":[{}]}}", self.gen, i, n, accts.iter().map(|x| format!("\"{x}\"")).collect::<Vec<_>>().join(","), cells.join(","))
        }).collect()
    }
}

#[derive(Deserialize)]
struct WireD { g: u64, k: u64, c: Vec<[u64; 2]> }
#[derive(Deserialize)]
struct WireS { g: u64, i: u64, n: u64, a: Vec<String>, c: Vec<[u64; 4]> }

/// What the UI reads: the wall as it stands, and the rules in force for it.
pub struct WallView { pub gen: u64, pub locked: bool, pub rev: u64, pub snap_at: u64, pub peer_at: u64, pub cells: Vec<(u16, u8, u8, String)> }

impl Client {
    /// (generation, locked, owner account) for a room, from the owner-signed style document of its community. A room we
    /// know nothing about has generation 0, is not locked and has no owner to defer to.
    pub(crate) fn wall_rules(&self, gid: &str) -> (u64, bool, String) {
        let Some(c) = self.st.groups.get(gid).and_then(|g| self.st.communities.get(&g.community)) else { return (0, false, String::new()) };
        let (gen, locked) = c.style.as_ref().map_or((0, false), |s| (s.wall.get(gid).copied().unwrap_or(0), s.locked.contains(gid)));
        (gen, locked, c.owner.clone())
    }
    fn wall_mut(&mut self, gid: &str) -> &mut Wall {
        let (gen, _, _) = self.wall_rules(gid);
        if !self.walls.contains_key(gid) && self.walls.len() >= WALL_MAX_WALLS {
            if let Some(old) = self.walls.iter().min_by_key(|(_, w)| w.touched).map(|(g, _)| g.clone()) { self.walls.remove(&old); }
        }
        let w = self.walls.entry(gid.to_string()).or_default();
        if gen > w.gen { w.sync_gen(gen); self.walls_dirty = true; self.wall_rev += 1; }
        w.touched = commons_proto::now_secs();
        w
    }

    /// Draw cells `(x, y, glyph, colour)` on a room's wall: applied here at once, and offered to the room best-effort (live
    /// events are never retried). Returns `true` when it was applied and offered. `false` means "ask again shortly": either the room cannot send
    /// right now and the cells were kept here only, or a stroke is still waiting to go out and nothing was changed (a second one would replace it).
    /// Refused when the wall is locked and we are not the owner.
    pub fn wall_draw(&mut self, gid: &str, cells: &[(u32, u32, u8, u8)]) -> Result<bool> { self.wall_draw_to(gid, cells, true) }

    /// As `wall_draw`; with `share` false the cells stay on this device (nobody is around to see them, so nothing is sent into the room).
    /// They reach others later in a snapshot when someone opens the room.
    pub fn wall_draw_to(&mut self, gid: &str, cells: &[(u32, u32, u8, u8)], share: bool) -> Result<bool> {
        self.check_chan(gid)?;
        if cells.is_empty() || cells.len() > WALL_BATCH { bail!("a stroke has 1 to {WALL_BATCH} cells"); }
        let mut flat = Vec::with_capacity(cells.len());
        for &(x, y, g, c) in cells {
            if x >= WALL_COLS || y >= WALL_ROWS || g as usize >= WALL_GLYPHS.len() || c > 1 { bail!("a cell is outside the wall or not allowed"); }
            flat.push([(y * WALL_COLS + x) as u64, g as u64 * 2 + c as u64]);
        }
        let (_, locked, owner) = self.wall_rules(gid);
        let me = self.my_account();
        if locked && me != owner { bail!("the owner has locked this wall"); }
        if share && self.live_would_wait(gid, "wall.d") { return Ok(false); }
        let w = self.wall_mut(gid);
        let (gen, clock) = (w.gen, w.clock + 1);
        for f in &flat { w.put(f[0] as u32, (f[1] / 2) as u8, (f[1] % 2) as u8, clock, &me); }
        self.walls_dirty = true; self.wall_rev += 1;
        let data = format!("{{\"g\":{},\"k\":{},\"c\":[{}]}}", gen, clock, flat.iter().map(|f| format!("[{},{}]", f[0], f[1])).collect::<Vec<_>>().join(","));
        if !share { return Ok(true); }
        self.live_send(gid, "wall.d", &data)
    }

    /// The wall as it stands (empty if we have none), with the owner's rules for it. Reads only, so the UI can poll it without
    /// waiting behind anything long: a wall whose generation is behind the owner's is shown cleared (it is reset for real the
    /// next time anything touches it).
    pub fn wall_view(&self, gid: &str) -> WallView {
        let (gen, locked, _) = self.wall_rules(gid);
        let snap_at = self.wall_snap_seen.get(gid).copied().unwrap_or(0);
        let peer_at = self.wall_peer_seen.get(gid).copied().unwrap_or(0);
        let rev = self.wall_rev + gen;
        match self.walls.get(gid).filter(|w| w.gen >= gen) {
            Some(w) => WallView { gen: w.gen, locked, rev, snap_at, peer_at, cells: w.cells.iter().map(|(p, c)| (*p, c.0, c.1, w.author(c).to_string())).collect() },
            None => WallView { gen, locked, rev, snap_at, peer_at, cells: vec![] },
        }
    }
    /// Snapshot chunks to hand to a member who asked: up to eight events, to be sent slowly.
    pub fn wall_snapshot(&mut self, gid: &str) -> Result<Vec<String>> {
        self.check_chan(gid)?;
        Ok(self.wall_mut(gid).snapshot())
    }

    /// A `wall.d` or `wall.s` event from `acct`. Everything in it is untrusted.
    pub(crate) fn take_wall(&mut self, gid: &str, acct: &str, kind: &str, data: &str) {
        let (gen, locked, owner) = self.wall_rules(gid);
        let from_owner = acct == owner;
        if acct != self.my_account() { self.wall_peer_seen.insert(gid.to_string(), commons_proto::now_secs()); }
        let mut changed = false;
        if kind == "wall.d" {
            let Ok(d) = serde_json::from_str::<WireD>(data) else { return };
            if d.g != gen || d.c.is_empty() || d.c.len() > WALL_BATCH || d.k == 0 || d.k > MAX_CLOCK { return; }
            if locked && !from_owner { return; }
            if d.c.iter().any(|c| c[0] >= WALL_CELLS as u64 || c[1] >= WALL_GLYPHS.len() as u64 * 2) { return; }
            let w = self.wall_mut(gid);
            if d.k > w.clock.saturating_add(MAX_CLOCK_STEP) { return; }
            for c in &d.c { changed |= w.put(c[0] as u32, (c[1] / 2) as u8, (c[1] % 2) as u8, d.k, acct); }
        } else if kind == "wall.s" {
            let Ok(s) = serde_json::from_str::<WireS>(data) else { return };
            if s.g != gen || s.n == 0 || s.n > WALL_SNAP_CHUNKS as u64 || s.i >= s.n || s.a.len() > 16 || s.c.is_empty() || s.c.len() > 400 { return; }
            if s.a.iter().any(|a| !is_acct(a)) { return; }
            if s.c.iter().any(|c| c[0] >= WALL_CELLS as u64 || c[1] >= WALL_GLYPHS.len() as u64 * 2 || c[2] == 0 || c[2] > MAX_CLOCK || c[3] >= s.a.len() as u64) { return; }
            let w = self.wall_mut(gid);
            for c in &s.c {
                // A snapshot is second-hand. While the wall is locked it may only fill cells that are still empty, so it cannot be used to draw.
                if locked && !from_owner && w.cells.contains_key(&(c[0] as u16)) { continue; }
                if c[2] > w.clock.saturating_add(MAX_CLOCK_STEP) { continue; }
                changed |= w.put(c[0] as u32, (c[1] / 2) as u8, (c[1] % 2) as u8, c[2], &s.a[c[3] as usize]);
            }
            self.wall_snap_seen.insert(gid.to_string(), commons_proto::now_secs());
        }
        if changed { self.walls_dirty = true; self.wall_rev += 1; }
    }

    /// Put the walls into the saved state, at most every `WALL_FLUSH_SECS` unless `force`d. Called by `save`.
    pub(crate) fn flush_walls(&mut self, force: bool) {
        if !self.walls_dirty { return; }
        let now = commons_proto::now_secs();
        if !force && now < self.walls_flushed + WALL_FLUSH_SECS { return; }
        self.st.walls = self.walls.clone();
        self.walls_dirty = false;
        self.walls_flushed = now;
    }
    #[doc(hidden)]
    pub fn debug_flush_walls(&mut self) { self.flush_walls(true); }
}

