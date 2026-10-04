//! The index's rows as the query engine's notes, for search, Bases and graph
//! filters.

use igneous_index::NoteRow;
use igneous_query::{LinkData, NoteData};

pub fn note_data(row: NoteRow) -> NoteData {
    let link = |path: igneous_core::VaultPath| LinkData {
        target: path.to_string(),
        path: Some(path),
    };
    let mut data = NoteData::new(row.path);
    data.text = row.text.into();
    data.size = row.size;
    data.ctime = row.ctime;
    data.mtime = row.mtime;
    data.properties = row.properties;
    data.tags = row.tags;
    data.links = row.links.into_iter().map(link).collect();
    data.embeds = row.embeds.into_iter().map(link).collect();
    data.backlinks = row.backlinks;
    data
}

impl crate::index::IndexService {
    /// The vault as query data, built once per index update and shared.
    pub async fn data(&self) -> std::sync::Arc<Vec<NoteData>> {
        if let Some(data) = self.cached_data() {
            return data;
        }
        let data = std::sync::Arc::new(self.note_data().await);
        self.cache_data(data.clone());
        data
    }

    /// Every file in the vault as query data. Building it reads every note,
    /// so it runs on the index's thread.
    pub async fn note_data(&self) -> Vec<NoteData> {
        self.query(|index| index.note_rows())
            .await
            .and_then(Result::ok)
            .map(|rows| rows.into_iter().map(note_data).collect())
            .unwrap_or_default()
    }
}
