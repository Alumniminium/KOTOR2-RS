//! The text an MCP client gets, sized for a model's context: one copy of each
//! message, no envelope, compact JSON, and observe as a few short lines per
//! object. Floats are rounded to two decimals everywhere.

use std::fmt::Write as _;

use k2_world::control::{Item, Member, Nearby, Observation, Quest, Reply};
use serde_json::{Map, Value};

/// Rounds a float to two decimals.
fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// The same JSON with every float rounded to two decimals. A whole number
/// prints as an integer.
pub fn round_floats(value: Value) -> Value {
    match value {
        Value::Number(n) if n.is_f64() => {
            let x = round2(n.as_f64().unwrap_or(0.0));
            if x.fract() == 0.0 && x.abs() < 9.0e15 {
                Value::from(x as i64)
            } else {
                serde_json::Number::from_f64(x).map_or(Value::Null, Value::Number)
            }
        }
        Value::Array(items) => Value::Array(items.into_iter().map(round_floats).collect()),
        Value::Object(fields) => Value::Object(fields.into_iter().map(|(k, v)| (k, round_floats(v))).collect()),
        other => other,
    }
}

/// `body`, then the game's messages for the reply. Each message appears once,
/// and a message equal to the reply's own message is left out.
pub fn with_feedback(body: String, reply: &Reply) -> String {
    let mut lines = vec![body];
    for line in &reply.feedback {
        if *line != reply.message && !lines.contains(line) {
            lines.push(line.clone());
        }
    }
    lines.join("\n")
}

/// A reply that succeeded, as text: its data as one line of JSON, or its
/// message when it has no data; then the game's messages.
pub fn plain(reply: &Reply) -> String {
    let body = match &reply.data {
        Some(data) if !data.is_null() => waited(reply, serde_json::to_string(&round_floats(data.clone())).unwrap_or_default()),
        _ if reply.message.is_empty() => "ok".to_owned(),
        _ => reply.message.clone(),
    };
    with_feedback(body, reply)
}

/// The body of a reply that waited for speech (its data has waited_s): the
/// message comes first, since it says how long and for whom. Other replies keep
/// their body as it is.
pub fn waited(reply: &Reply, body: String) -> String {
    if reply.message.is_empty() || reply.data.as_ref().is_none_or(|data| data.get("waited_s").is_none()) {
        return body;
    }
    format!("{}\n{body}", reply.message)
}

/// observe as text: the open window or screen first when `open` is set, then a
/// header line, one line per party member and one per interactable object nearby
/// (nearest first, with its direction from the leader), then the conversation,
/// combat, journal, inventory and level-ups when they are set. Empty and default
/// fields are left out. verbose adds the positions, the Force powers of each party
/// member and the text of each journal entry. None when the data is not an
/// observation.
pub fn observe(data: &Value, verbose: bool) -> Option<String> {
    let obs: Observation = serde_json::from_value(data.clone()).ok()?;
    let leader = obs.leader.and_then(|id| obs.party.iter().find(|m| m.id == id));
    let mut lines: Vec<String> = data["open"].as_str().map(str::to_owned).into_iter().collect();
    lines.push(header(&obs));
    lines.extend(obs.party.iter().map(|m| member_line(m, verbose)));
    lines.extend(obs.nearby.iter().filter(|n| interactable(n)).map(|n| nearby_line(n, leader, verbose)));
    if let Some(conv) = &obs.conversation {
        lines.push(format!("conversation {} speaker={} \"{}\": {}", conv.dialog, conv.speaker.id, conv.speaker.name, conv.text));
        lines.extend(conv.replies.iter().map(|r| format!("  {}: {}", r.index, r.text)));
    }
    if obs.combat.in_combat {
        let engaged: Vec<String> = obs.combat.engaged.iter().map(|o| format!("{} \"{}\"", o.id, o.name)).collect();
        lines.push(format!("combat engaged=[{}]", engaged.join(", ")));
    }
    lines.extend(obs.journal.iter().map(|q| journal_line(q, verbose)));
    lines.extend(obs.inventory.iter().map(item_line));
    lines.extend(obs.level_ups.iter().map(|l| format!("level_up {} \"{}\" -> {}", l.creature, l.name, l.new_level)));
    if obs.credits != 0 {
        lines.push(format!("credits {}", obs.credits));
    }
    Some(lines.join("\n"))
}

/// game_status as data. Unless verbose, the log filter, the socket path and the
/// physical window sizes are left out, each build is shown by its stamp, the
/// last build is left out while a game runs (it is the running build), and an
/// exit's time is left out.
pub fn status(data: &Value, verbose: bool) -> Value {
    if verbose {
        return round_floats(data.clone());
    }
    let mut data = data.clone();
    let running = !data["run"].is_null();
    if let Some(game) = data.get_mut("game").and_then(Value::as_object_mut) {
        game.remove("log_filter");
        game.retain(|_, v| !v.is_null());
        if let Some(window) = game.get_mut("window").and_then(Value::as_object_mut) {
            window.remove("physical_width");
            window.remove("physical_height");
        }
    }
    if let Some(run) = data.get_mut("run").and_then(Value::as_object_mut) {
        run.remove("socket");
        shorten_build(run, "build");
    }
    if let Some(top) = data.as_object_mut() {
        if running {
            top.remove("last_build");
        } else {
            shorten_build(top, "last_build");
        }
        if let Some(exit) = top.get_mut("last_exit").and_then(Value::as_object_mut) {
            exit.remove("at_unix_ms");
        }
    }
    round_floats(data)
}

/// Replaces the build record under `key` with its stamp.
fn shorten_build(map: &mut Map<String, Value>, key: &str) {
    let stamp = map.get(key).and_then(|build| build.get("stamp")).cloned();
    if let Some(stamp) = stamp {
        map.insert(key.to_owned(), stamp);
    }
}

/// Joins the non-empty parts with spaces.
fn join_parts(parts: Vec<String>) -> String {
    parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" ")
}

fn header(obs: &Observation) -> String {
    let mut parts = vec![obs.module.clone(), obs.area.clone(), format!("t={:.2}", obs.time)];
    if let Some(leader) = obs.leader {
        parts.push(format!("leader={leader}"));
    }
    if obs.paused {
        parts.push("paused".to_owned());
    }
    if obs.game_over {
        parts.push("game-over".to_owned());
    }
    join_parts(parts)
}

fn member_line(m: &Member, verbose: bool) -> String {
    let name = if m.name.is_empty() { String::new() } else { format!("\"{}\"", m.name) };
    let mut parts = vec![m.id.to_string(), m.tag.clone(), name, format!("lvl={}", m.level), format!("hp={}/{}", m.hp, m.max_hp)];
    if m.max_fp > 0 {
        parts.push(format!("fp={}/{}", m.fp, m.max_fp));
    }
    if m.next_level_xp > 0 {
        parts.push(format!("xp={}/{}", m.xp, m.next_level_xp));
    }
    if verbose {
        let [x, y, z] = m.position;
        parts.push(format!("pos=({x:.2},{y:.2},{z:.2})"));
    }
    for (flag, on) in [("leader", m.leader), ("dead", m.dead), ("combat", m.in_combat)] {
        if on {
            parts.push(flag.to_owned());
        }
    }
    if !m.conditions.is_empty() {
        parts.push(format!("cond=[{}]", m.conditions.join(", ")));
    }
    if !m.equipment.is_empty() {
        let gear: Vec<String> = m.equipment.iter().map(|e| format!("{}:{}", e.slot, e.name)).collect();
        parts.push(format!("gear=[{}]", gear.join(", ")));
    }
    if verbose && !m.powers.is_empty() {
        let powers: Vec<String> = m.powers.iter().map(|p| format!("{} {}", p.id, p.name)).collect();
        parts.push(format!("powers=[{}]", powers.join(", ")));
    }
    join_parts(parts)
}

/// What observe lists: doors that can be used, placeables that can be used or
/// talked to, creatures that are alive or talk, items on the ground, and trapped
/// triggers. Scenery (chairs, desks) and helpers (inv_talker, k_trans_abort) are
/// left out.
fn interactable(n: &Nearby) -> bool {
    match n.kind.as_str() {
        "door" => n.useable,
        "placeable" => n.useable || n.conversation,
        "creature" => n.useable || n.conversation,
        "item" => true,
        "trigger" => n.trapped,
        _ => false,
    }
}

/// The eight sectors around the leader, from straight ahead and counter-clockwise
/// (to the left).
const SECTORS: [&str; 8] = [
    "in front of you",
    "ahead and to your left",
    "to your left",
    "behind you and to your left",
    "behind you",
    "behind you and to your right",
    "to your right",
    "ahead and to your right",
];

/// Where `to` is from the leader, in words: the distance in metres (0.1), the
/// sector from the leader's facing, and above or below when the height differs
/// by more than 1.5 m.
fn direction(leader: &Member, to: [f32; 3], distance: f32) -> String {
    let turn = (to[1] - leader.position[1]).atan2(to[0] - leader.position[0]) - leader.facing;
    let turn = (turn + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    let sector = ((turn / std::f32::consts::FRAC_PI_4).round() as i32).rem_euclid(8) as usize;
    let mut text = format!("{distance:.1} m {}", SECTORS[sector]);
    let rise = to[2] - leader.position[2];
    if rise > 1.5 {
        text.push_str(", above");
    } else if rise < -1.5 {
        text.push_str(", below");
    }
    text
}

/// One interactable object: `id name (kind, flags) - distance and direction`.
/// Without the leader's position the direction is left out. verbose adds the
/// raw position.
fn nearby_line(n: &Nearby, leader: Option<&Member>, verbose: bool) -> String {
    let name = if n.name.is_empty() { n.tag.clone() } else { n.name.clone() };
    let mut flags: Vec<&str> = Vec::new();
    if n.kind == "door" {
        match n.open {
            Some(true) => flags.push("open"),
            Some(false) => flags.push("closed"),
            None => {}
        }
    }
    if n.locked == Some(true) {
        flags.push("locked");
    }
    if n.conversation {
        flags.push("talks");
    }
    if n.hostile == Some(true) {
        flags.push("hostile");
    }
    if n.dead == Some(true) {
        flags.push("dead");
    }
    if n.trapped {
        flags.push("trapped");
    }
    if n.scripted_use {
        flags.push("scripted");
    }
    let kind = if flags.is_empty() { n.kind.clone() } else { format!("{}, {}", n.kind, flags.join(", ")) };
    let place = match leader {
        Some(leader) => format!(" - {}", direction(leader, n.position, n.distance)),
        None => format!(" - {:.1} m", n.distance),
    };
    let head = if name.is_empty() { n.id.to_string() } else { format!("{} {name}", n.id) };
    let mut line = format!("{head} ({kind}){place}");
    if verbose {
        let [x, y, z] = n.position;
        let _ = write!(line, " at=({x:.2},{y:.2},{z:.2})");
    }
    line
}

fn journal_line(q: &Quest, verbose: bool) -> String {
    let mut line = format!("quest \"{}\" state={}", q.name, q.state);
    if verbose && !q.text.is_empty() {
        let _ = write!(line, ": {}", q.text);
    }
    line
}

fn item_line(i: &Item) -> String {
    let mut parts = vec![format!("item {} \"{}\"", i.id, i.name)];
    if i.stack != 1 {
        parts.push(format!("x{}", i.stack));
    }
    if i.value > 0 {
        parts.push(format!("value={}", i.value));
    }
    if i.plot {
        parts.push("plot".to_owned());
    }
    join_parts(parts)
}

#[cfg(test)]
mod tests;
