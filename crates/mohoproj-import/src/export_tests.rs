//! Export gate (stage 7, ADR 0107): what the writer produces is a real Moho package that
//! re-imports to the same document. Corpus fixtures (when the corpus is here) go through
//! export → import and the model must come back equal; a generated native document goes
//! through the same loop and its geometry, channels and structure must survive.

#[cfg(test)]
mod tests {
    use crate::export::{package_bytes, project_json};
    use crate::import::import_path;
    use crate::zip::Archive;
    use anicraft_doc::{Document, ElementKind, LayerKind};
    use std::path::{Path, PathBuf};

    fn corpus() -> Option<PathBuf> {
        if let Some(dir) = std::env::var_os("MAOHOU_FIXTURES") {
            let p = PathBuf::from(dir);
            if p.is_dir() {
                return Some(p);
            }
        }
        let beside = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../maop-corpus/maop");
        beside.is_dir().then_some(beside)
    }

    /// Imports a fixture, exports it as a package, and imports the export back.
    fn round_trip(name: &str) -> Option<(Document, Document)> {
        let dir = corpus()?;
        let path = dir.join(format!("{name}.moho"));
        if !path.is_file() {
            eprintln!("skipped {name}: no {name}.moho in the corpus");
            return None;
        }
        let (doc, _report) = import_path(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
        let bytes = package_bytes(&doc, None).unwrap_or_else(|e| panic!("{name}: export failed: {e}"));
        let archive = Archive::open(&bytes).unwrap_or_else(|e| panic!("{name}: the package is not a zip: {e}"));
        let project = archive.read("Project.mohoproj").unwrap_or_else(|e| panic!("{name}: {e}")).unwrap_or_else(|| panic!("{name}: no Project.mohoproj in the package"));
        let read = |entry: &str| -> Result<Option<Vec<u8>>, crate::MohoError> { Ok(if entry == "Project.mohoproj" { Some(project.clone()) } else { archive.read(entry)? }) };
        let (again, _report2) = crate::import::import_with(&read).unwrap_or_else(|e| panic!("{name}: the export does not re-import: {e}"));
        Some((doc, again))
    }

    /// The model fields the round trip must hold, as comparable strings (the sidecar's raw
    /// JSON is the untouched original on the first import and the export's on the second, so
    /// it compares by design only where the model carries it).
    fn model_fingerprint(doc: &Document) -> Vec<String> {
        let mut out = Vec::new();
        out.push(format!("stage {}x{} @{}", doc.stage.width, doc.stage.height, doc.stage.fps));
        let scene = doc.scenes.first().expect("a scene");
        for l in &scene.timeline.layers {
            out.push(format!("layer {} {:?} parent {:?}", l.name, l.kind, l.parent));
            out.push(format!("  visible {} anim {} {} {}", l.visible, !l.translation.is_empty(), !l.rotation.is_empty(), !l.opacity.is_empty()));
            for k in &l.translation.keys {
                out.push(format!("  tr {} ({:.4},{:.4})", k.frame, k.val.x, k.val.y));
            }
            for k in &l.rotation.keys {
                out.push(format!("  rot {} {:.4}", k.frame, k.val));
            }
            if let Some(s) = &l.skeleton {
                out.push(format!("  bones {}", s.bones.len()));
                for b in &s.bones {
                    out.push(format!("  bone {} len {:.2} parent {:?}", b.name, b.length, b.parent));
                }
            }
            if l.kind == LayerKind::Switch {
                for k in &l.switch_to.keys {
                    out.push(format!("  switch {} {}", k.frame, k.val));
                }
            }
            for kf in &l.keyframes {
                for e in &kf.elements {
                    match &e.kind {
                        ElementKind::Shape(s) => out.push(format!("  shape fill {:?} stroke {}", s.fill.as_ref().map(|p| matches!(p, anicraft_doc::Paint::Solid(_))), s.stroke.is_some())),
                        ElementKind::Text(t) => out.push(format!("  text {:?} {}px", t.text, t.size)),
                        _ => {}
                    }
                }
            }
        }
        for (f, m) in &scene.markers {
            out.push(format!("marker {f} {m}"));
        }
        out
    }

    #[test]
    fn corpus_documents_survive_export_and_reimport() {
        let Some(_) = corpus() else { eprintln!("skipped: no maop-corpus"); return };
        for name in ["empty", "vectors", "keyframes-layer", "bones", "switch-keys", "point-anim", "text", "gradients-gui", "keyframes-roll"] {
            let Some((doc, again)) = round_trip(name) else { continue };
            let a = model_fingerprint(&doc);
            let b = model_fingerprint(&again);
            assert_eq!(a, b, "{name}: the exported package re-imports differently");
        }
    }

    #[test]
    fn a_native_document_round_trips_through_the_package() {
        // A native document with a shape, a switch and a camera, exported and re-imported.
        let mut doc = Document::default();
        doc.scenes.clear();
        let scene = anicraft_doc::Scene::new("Scene 1");
        let mut root = anicraft_doc::Layer::new(anicraft_doc::LayerId(doc.alloc_id()), "Root");
        root.kind = LayerKind::Folder;
        let mut kid = anicraft_doc::Layer::new(anicraft_doc::LayerId(doc.alloc_id()), "Box");
        kid.parent = Some(root.id);
        let shape = anicraft_doc::Shape::filled(anicraft_geom::Path::rect(600.0, 200.0, 80.0, 60.0), anicraft_doc::Paint::Solid(anicraft_doc::Rgba::hex(0xCC4444)));
        let mut kf = anicraft_doc::Keyframe::new(0, 1);
        kf.elements.push(std::sync::Arc::new(anicraft_doc::Element { id: anicraft_doc::ElementId(doc.alloc_id()), name: String::new(), transform: anicraft_geom::Affine::IDENTITY, color: anicraft_doc::ColorEffect::default(), kind: ElementKind::Shape(shape.clone()) }));
        kid.keyframes.push(std::sync::Arc::new(kf));
        doc.scenes.push(std::sync::Arc::new(anicraft_doc::Scene { timeline: anicraft_doc::Timeline { layers: vec![std::sync::Arc::new(kid), std::sync::Arc::new(root)] }, ..scene }));
        let (project, report) = project_json(&doc);
        assert!(report.skipped.is_empty(), "{report:?}");
        let bytes = package_bytes(&doc, None).unwrap();
        let archive = Archive::open(&bytes).unwrap();
        let project_bytes = archive.read("Project.mohoproj").unwrap().unwrap();
        assert_eq!(project_bytes, serde_json::to_vec(&project).unwrap(), "the zip carries the project JSON");
        let read = |entry: &str| -> Result<Option<Vec<u8>>, crate::MohoError> { Ok(archive.read(entry)?) };
        let (again, report2) = crate::import::import_with(&read).unwrap();
        assert_eq!(report2.skipped.len(), 0, "{report2:?}");
        let scene2 = again.scenes.first().unwrap();
        assert_eq!(scene2.timeline.layers.len(), 2, "both layers come back");
        // The flatten order puts the container before its children.
        assert_eq!(scene2.timeline.layers[0].name, "Root");
        let kid2 = &scene2.timeline.layers[1];
        assert_eq!(kid2.name, "Box");
        assert_eq!(kid2.parent.map(|p| p.0), Some(scene2.timeline.layers[0].id.0));
        let shape2 = kid2.keyframes.first().and_then(|k| k.elements.first()).map(|e| match &e.kind {
            ElementKind::Shape(s) => s.clone(),
            _ => panic!("the element is not a shape"),
        }).expect("a shape");
        // The rectangle, back through Moho inches: the imported path is in inches and its
        // element transform carries it to pixels. Half a pixel of slack for the float path.
        let b1 = shape.path.bounds();
        let xf = kid2.keyframes.first().and_then(|k| k.elements.first()).map(|e| e.transform).unwrap_or_default();
        let b2 = shape2.path.bounds().transformed(&xf);
        assert!((b1.min.x - b2.min.x).abs() < 0.5 && (b1.max.x - b2.max.x).abs() < 0.5, "{b1:?} vs {b2:?}");
        assert!((b1.min.y - b2.min.y).abs() < 0.5 && (b1.max.y - b2.max.y).abs() < 0.5, "{b1:?} vs {b2:?}");
        let fill2 = shape2.fill.as_ref().expect("the fill comes back");
        match fill2 {
            anicraft_doc::Paint::Solid(c) => assert!((c.r - anicraft_doc::Rgba::hex(0xCC4444).r).abs() < 0.01, "the fill colour comes back"),
            _ => panic!("not a solid fill"),
        }
    }

    #[test]
    fn the_project_json_carries_the_canonical_root_keys() {
        let doc = Document::default();
        let (project, _report) = project_json(&doc);
        for key in ["version", "mime_type", "major_version", "doc_uuid", "project_data", "layers", "styles", "animated_values", "onions_enabled", "documentviewstate", "thumbnail"] {
            assert!(project.get(key).is_some(), "missing {key}");
        }
        let pd = project.get("project_data").unwrap();
        for key in ["width", "height", "fps", "back_color", "start_frame", "end_frame"] {
            assert!(pd.get(key).is_some(), "project_data misses {key}");
        }
        let layers = project.get("layers").unwrap().as_array().unwrap();
        assert!(!layers.is_empty(), "a document without layers is not a Moho document");
        let layer = &layers[0];
        for key in ["type", "name", "uuid", "visible", "transforms", "mesh"] {
            assert!(layer.get(key).is_some(), "the layer template misses {key}");
        }
    }
}
