//! Switch commands (ADR 0105): pick which variant a switch layer shows at a frame, clear a
//! pick, and read a switch back. The variants are the switch's direct child layers; the
//! pick is a key on the layer's selector channel, so switching animates like any channel.

use crate::params::*;
use crate::{CommandSpec, EngineError, Result, Session};
use anicraft_doc::{LayerId, LayerKind};
use serde_json::{Value, json};

pub fn specs() -> Vec<CommandSpec> {
    let c = |id, label, menu, shortcut, params, mutates, run| CommandSpec { id, label, menu, shortcut, params, mutates, run };
    vec![
        c("switch.set_variant", "Switch Variant", "Insert/Timeline", None, "{name (a variant layer of the switch), frame?, layer?} (keys the switch to show that variant at the frame)", true, set_variant),
        c("switch.delete_key", "Delete Switch Key", "Modify/Timeline", None, "{frame?, layer?} (removes the variant key at the frame; earlier keys hold again)", true, delete_key),
        c("switch.info", "Switch Info", "", None, "{layer?, frame?} → the variants, the key frames and the selection at the frame", false, info),
    ]
}

/// The switch layer the command targets: `layer` when it is a switch, else the enclosing
/// switch above the active layer (like a bone command resolving its rig).
fn switch_of(s: &Session, layer: Option<u64>) -> Result<LayerId> {
    let mut lid = layer.map(LayerId).unwrap_or(s.layer);
    let tl = s.doc.timeline(s.scene);
    for _ in 0..=anicraft_doc::MAX_LAYER_DEPTH {
        let Some(l) = tl.layer(lid) else { break };
        if l.kind == LayerKind::Switch {
            return Ok(l.id);
        }
        match l.parent {
            Some(p) => lid = p,
            None => break,
        }
    }
    Err(EngineError::State("no switch layer here; a switch holds variant layers (Import a Moho switch, or switch.set_variant on one)".into()))
}

/// The switch's variant names, in child order.
fn variants_of(s: &Session, sw: LayerId) -> Vec<String> {
    s.doc.timeline(s.scene).layers.iter().filter(|l| l.parent == Some(sw)).map(|l| l.name.clone()).collect()
}

fn set_variant(s: &mut Session, p: &Value) -> Result<Value> {
    let sw = switch_of(s, opt_u64(p, "layer")?)?;
    let frame = opt_u32(p, "frame")?.unwrap_or(s.frame);
    if frame > crate::MAX_FRAME {
        return Err(EngineError::Param("frame is past the timeline".into()));
    }
    let name = opt_str(p, "name")?.ok_or_else(|| EngineError::Param("name (a variant layer) is required".into()))?;
    let variants = variants_of(s, sw);
    if !variants.iter().any(|v| v == name) {
        return Err(EngineError::Param(format!("'{name}' is not a variant of this switch ({})", variants.join(", "))));
    }
    let tl = s.doc.timeline_mut(s.scene);
    let layer = tl.layer_mut(sw).ok_or_else(|| EngineError::State("the switch layer is gone".into()))?;
    layer.switch_to.set_key(frame, name.to_string());
    Ok(json!({"switch": sw.0, "variant": name, "frame": frame}))
}

fn delete_key(s: &mut Session, p: &Value) -> Result<Value> {
    let sw = switch_of(s, opt_u64(p, "layer")?)?;
    let frame = opt_u32(p, "frame")?.unwrap_or(s.frame);
    let gone = s.doc
        .timeline_mut(s.scene)
        .layer_mut(sw)
        .ok_or_else(|| EngineError::State("the switch layer is gone".into()))?
        .switch_to
        .remove_key(frame);
    if !gone {
        return Err(EngineError::State(format!("no switch key at frame {frame}")));
    }
    Ok(json!({"switch": sw.0, "deleted": frame}))
}

fn info(s: &mut Session, p: &Value) -> Result<Value> {
    let sw = switch_of(s, opt_u64(p, "layer")?)?;
    let frame = opt_u32(p, "frame")?.unwrap_or(s.frame);
    let tl = s.doc.timeline(s.scene);
    let l = tl.layer(sw).ok_or_else(|| EngineError::State("the switch layer is gone".into()))?;
    let selection = l.switch_selection(frame);
    let keys: Vec<(u32, &String)> = l.switch_to.keys.iter().map(|k| (k.frame, &k.val)).collect();
    Ok(json!({
        "switch": sw.0,
        "variants": variants_of(s, sw),
        "keys": keys.iter().map(|(f, v)| json!({"frame": f, "variant": v})).collect::<Vec<_>>(),
        "selection": selection,
    }))
}
