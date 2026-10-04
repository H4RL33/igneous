//! What the graph view shows: notes, and optionally attachments, tags and
//! unresolved links, with the links between them, built from the index and
//! the settings in `.igneous/graph.json`.
//!
//! Filters and colour groups use Obsidian's search syntax, matched against
//! each file with igneous-query.

use std::collections::HashMap;

use igneous_core::VaultPath;
use igneous_core::settings::GraphSettings;
use igneous_graph::{Graph, Node, NodeKind};
use igneous_index::{EdgeTarget, GraphEdge};
use igneous_query::NoteData;
use igneous_query::search::{Matcher, SearchOptions};

/// Everything the graph is built from: each file's data and every link.
#[derive(Debug, Clone, Default)]
pub struct GraphSource {
    pub notes: Vec<NoteData>,
    pub edges: Vec<GraphEdge>,
}

/// The forces in `settings`, for the layout.
pub fn forces(settings: &GraphSettings) -> igneous_graph::Forces {
    igneous_graph::Forces {
        center: settings.center_force as f32,
        repel: settings.repel_force as f32,
        link_strength: settings.link_force as f32,
        link_distance: settings.link_distance as f32,
    }
}

/// The graph, and for each node the file it stands for (none for tags and
/// unresolved links).
#[derive(Debug, Clone, Default)]
pub struct GraphModel {
    pub graph: Graph,
    pub paths: Vec<Option<VaultPath>>,
    /// Search queries that didn't parse (the filter or a group's), to show.
    pub errors: Vec<String>,
}

impl GraphModel {
    /// The node standing for `path`.
    pub fn find(&self, path: &VaultPath) -> Option<usize> {
        self.paths.iter().position(|p| p.as_ref() == Some(path))
    }

    /// The neighbourhood of `path`, `depth` links deep (for the inspector).
    pub fn local(&self, path: &VaultPath, depth: u8) -> Option<GraphModel> {
        let root = self.find(path)?;
        let (graph, map) = igneous_graph::local(&self.graph, root, depth);
        Some(GraphModel {
            graph,
            paths: map.into_iter().map(|i| self.paths[i].clone()).collect(),
            errors: Vec::new(),
        })
    }
}

/// Builds the graph from every file's data and every link.
pub fn build(notes: &[NoteData], edges: &[GraphEdge], settings: &GraphSettings) -> GraphModel {
    let mut errors = Vec::new();
    let options = SearchOptions::default();
    let filter = match settings.filter.trim() {
        "" => None,
        query => match Matcher::parse(query, options) {
            Ok(matcher) => Some(matcher),
            Err(e) => {
                errors.push(format!("Filter: {e}"));
                None
            }
        },
    };
    let groups: Vec<Option<Matcher>> = settings
        .groups
        .iter()
        .map(|g| match g.query.trim() {
            "" => None,
            query => match Matcher::parse(query, options) {
                Ok(matcher) => Some(matcher),
                Err(e) => {
                    errors.push(format!("Group “{query}”: {e}"));
                    None
                }
            },
        })
        .collect();

    let mut graph = Graph::default();
    let mut paths = Vec::new();
    let mut by_path: HashMap<&VaultPath, u32> = HashMap::new();
    for note in notes {
        let kind = if note.is_note() {
            NodeKind::Note
        } else if settings.show_attachments && is_attachment(&note.path) {
            NodeKind::Attachment
        } else {
            continue;
        };
        if let Some(filter) = &filter
            && filter.matches(note).is_none()
        {
            continue;
        }
        let label = if kind == NodeKind::Note {
            note.path.stem().to_owned()
        } else {
            note.path.file_name().to_owned()
        };
        let mut node = Node::new(note.path.to_string(), label, kind);
        node.group = groups
            .iter()
            .position(|g| g.as_ref().is_some_and(|m| m.matches(note).is_some()))
            .map(|i| i as u16);
        by_path.insert(&note.path, graph.nodes.len() as u32);
        graph.nodes.push(node);
        paths.push(Some(note.path.clone()));
    }

    let mut extra: HashMap<String, u32> = HashMap::new();
    let mut add_extra =
        |graph: &mut Graph, paths: &mut Vec<Option<VaultPath>>, id: String, label: String, kind| {
            *extra.entry(id.clone()).or_insert_with(|| {
                graph.nodes.push(Node::new(id, label, kind));
                paths.push(None);
                graph.nodes.len() as u32 - 1
            })
        };
    for edge in edges {
        let Some(&source) = by_path.get(&edge.source) else {
            continue;
        };
        let target = match &edge.target {
            EdgeTarget::File(path) => match by_path.get(path) {
                Some(&target) => target,
                None => continue,
            },
            EdgeTarget::Unresolved(target) if settings.show_unresolved => add_extra(
                &mut graph,
                &mut paths,
                format!("unresolved:{}", target.to_lowercase()),
                target.clone(),
                NodeKind::Unresolved,
            ),
            EdgeTarget::Unresolved(_) => continue,
        };
        graph.edges.push((source, target));
    }
    if settings.show_tags {
        for note in notes {
            let Some(&source) = by_path.get(&note.path) else {
                continue;
            };
            for tag in &note.tags {
                let target = add_extra(
                    &mut graph,
                    &mut paths,
                    format!("#{}", tag.to_lowercase()),
                    format!("#{tag}"),
                    NodeKind::Tag,
                );
                graph.edges.push((source, target));
            }
        }
    }

    let mut model = GraphModel {
        graph,
        paths,
        errors,
    };
    if !settings.show_orphans {
        drop_orphans(&mut model);
    }
    model
}

/// Files that aren't notes but can be linked: images, PDFs, Bases and so on.
fn is_attachment(path: &VaultPath) -> bool {
    path.extension().is_some_and(|e| e != "md")
}

fn drop_orphans(model: &mut GraphModel) {
    let degrees = model.graph.degrees();
    let mut remap = vec![u32::MAX; degrees.len()];
    let mut nodes = Vec::new();
    let mut paths = Vec::new();
    for (i, degree) in degrees.iter().enumerate() {
        if *degree > 0 {
            remap[i] = nodes.len() as u32;
            nodes.push(model.graph.nodes[i].clone());
            paths.push(model.paths[i].clone());
        }
    }
    let edges = model
        .graph
        .edges
        .iter()
        .filter(|&&(a, b)| a != b)
        .map(|&(a, b)| (remap[a as usize], remap[b as usize]))
        .collect();
    model.graph = Graph { nodes, edges };
    model.paths = paths;
}

#[cfg(test)]
mod tests {
    use super::*;
    use igneous_core::settings::GraphGroup;

    fn p(s: &str) -> VaultPath {
        VaultPath::new(s).unwrap()
    }

    fn fixture() -> (Vec<NoteData>, Vec<GraphEdge>) {
        let mut home = NoteData::from_text(p("Home.md"), "# Home\n#start\n");
        home.tags = vec!["start".into()];
        let roadmap = NoteData::from_text(p("Projects/Roadmap.md"), "plans");
        let lonely = NoteData::from_text(p("Lonely.md"), "nobody links here");
        let image = NoteData::new(p("Attachments/diagram.png"));
        let edges = vec![
            GraphEdge {
                source: p("Home.md"),
                target: EdgeTarget::File(p("Projects/Roadmap.md")),
                embed: false,
            },
            GraphEdge {
                source: p("Home.md"),
                target: EdgeTarget::File(p("Attachments/diagram.png")),
                embed: true,
            },
            GraphEdge {
                source: p("Projects/Roadmap.md"),
                target: EdgeTarget::Unresolved("Someday".into()),
                embed: false,
            },
        ];
        (vec![home, roadmap, lonely, image], edges)
    }

    fn labels(model: &GraphModel) -> Vec<&str> {
        model.graph.nodes.iter().map(|n| n.label.as_str()).collect()
    }

    #[test]
    fn notes_and_their_links() {
        let (notes, edges) = fixture();
        let model = build(&notes, &edges, &GraphSettings::default());
        assert_eq!(labels(&model), ["Home", "Roadmap", "Lonely"]);
        assert_eq!(model.graph.edges, [(0, 1)]);
        assert_eq!(model.find(&p("Lonely.md")), Some(2));
    }

    #[test]
    fn optional_kinds() {
        let (notes, edges) = fixture();
        let settings = GraphSettings {
            show_tags: true,
            show_attachments: true,
            show_unresolved: true,
            ..GraphSettings::default()
        };
        let model = build(&notes, &edges, &settings);
        assert_eq!(
            labels(&model),
            [
                "Home",
                "Roadmap",
                "Lonely",
                "diagram.png",
                "Someday",
                "#start"
            ]
        );
        let kinds: Vec<NodeKind> = model.graph.nodes.iter().map(|n| n.kind).collect();
        assert_eq!(kinds[3], NodeKind::Attachment);
        assert_eq!(kinds[4], NodeKind::Unresolved);
        assert_eq!(kinds[5], NodeKind::Tag);
        assert_eq!(model.graph.edges.len(), 4);
        assert_eq!(model.paths[4], None);
    }

    #[test]
    fn filters_orphans_and_groups() {
        let (notes, edges) = fixture();
        let settings = GraphSettings {
            filter: "-path:Lonely".into(),
            ..GraphSettings::default()
        };
        assert_eq!(
            labels(&build(&notes, &edges, &settings)),
            ["Home", "Roadmap"]
        );

        let settings = GraphSettings {
            show_orphans: false,
            ..GraphSettings::default()
        };
        let model = build(&notes, &edges, &settings);
        assert_eq!(labels(&model), ["Home", "Roadmap"]);
        assert_eq!(model.graph.edges, [(0, 1)]);

        let settings = GraphSettings {
            groups: vec![
                GraphGroup {
                    query: "path:Projects".into(),
                    color: None,
                },
                GraphGroup {
                    query: "tag:#start".into(),
                    color: None,
                },
            ],
            ..GraphSettings::default()
        };
        let model = build(&notes, &edges, &settings);
        let groups: Vec<Option<u16>> = model.graph.nodes.iter().map(|n| n.group).collect();
        assert_eq!(groups, [Some(1), Some(0), None]);

        let settings = GraphSettings {
            filter: "/(/".into(),
            ..GraphSettings::default()
        };
        let model = build(&notes, &edges, &settings);
        assert_eq!(model.errors.len(), 1);
        assert_eq!(model.graph.len(), 3);
    }

    #[test]
    fn local_neighbourhood() {
        let (notes, edges) = fixture();
        let settings = GraphSettings {
            show_unresolved: true,
            ..GraphSettings::default()
        };
        let model = build(&notes, &edges, &settings);
        let local = model.local(&p("Home.md"), 1).unwrap();
        assert_eq!(labels(&local), ["Home", "Roadmap"]);
        let local = model.local(&p("Home.md"), 2).unwrap();
        assert_eq!(labels(&local), ["Home", "Roadmap", "Someday"]);
        assert!(model.local(&p("Missing.md"), 1).is_none());
    }
}
