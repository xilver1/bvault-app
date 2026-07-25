//! PDB (DeviceSQL) database generation
//!
//! The export.pdb file is a little-endian database with a specific page structure.
//! This module generates valid PDB files that CDJs can read.
//!
//! Reference: https://djl-analysis.deepsymmetry.org/rekordbox-export-analysis/exports.html

use std::collections::HashMap;

use crate::error::Result;
use crate::page::{PageBuilder, IndexPageBuilder, PageType, TablePointer, FileHeader, PAGE_SIZE};
use crate::string::{encode_string, encode_isrc};
use crate::track::TrackAnalysis;

/// Row subtypes for offset size determination
const SUBTYPE_NEAR: u16 = 0x0060; // 1-byte offsets (artist, album short)
const SUBTYPE_FAR: u16 = 0x0064;  // 2-byte offsets (artist, album long)
const SUBTYPE_TRACK: u16 = 0x0024; // Track rows always use 2-byte offsets

/// High-level database builder
pub struct PdbBuilder {
    tracks: Vec<TrackInfo>,
    artists: HashMap<String, u32>,
    albums: HashMap<(String, u32), u32>, // (album_name, artist_id) -> album_id
    genres: HashMap<String, u32>,
    labels: HashMap<String, u32>,
    keys: HashMap<u8, u32>, // rekordbox_key_id -> row_id
    playlists: Vec<PlaylistInfo>,
    artworks: HashMap<String, u32>, // artwork_path -> artwork_id
    next_artist_id: u32,
    next_album_id: u32,
    next_genre_id: u32,
    next_label_id: u32,
    next_key_id: u32,
    next_artwork_id: u32,
}

/// Internal track representation
struct TrackInfo {
    analysis: TrackAnalysis,
    artist_id: u32,
    album_id: u32,
    genre_id: u32,
    label_id: u32,
    key_id: u32,
    artwork_id: u32,
    analyze_path: String,
}

/// Playlist information
pub struct PlaylistInfo {
    pub id: u32,
    pub parent_id: u32,
    pub name: String,
    pub is_folder: bool,
    pub sort_order: u32,
    pub track_ids: Vec<u32>,
}

impl PdbBuilder {
    pub fn new() -> Self {
        Self {
            tracks: Vec::new(),
            artists: HashMap::new(),
            albums: HashMap::new(),
            genres: HashMap::new(),
            labels: HashMap::new(),
            keys: HashMap::new(),
            playlists: Vec::new(),
            artworks: HashMap::new(),
            next_artist_id: 1,
            next_album_id: 1,
            next_genre_id: 1,
            next_label_id: 1,
            next_key_id: 1,
            next_artwork_id: 1,
        }
    }
    
    /// Add a track and return its ID
    pub fn add_track(&mut self, analysis: &TrackAnalysis, analyze_path: &str) -> u32 {
        self.add_track_with_artwork(analysis, analyze_path, None)
    }

    /// Add a track with optional artwork path and return its ID
    pub fn add_track_with_artwork(&mut self, analysis: &TrackAnalysis, analyze_path: &str, artwork_path: Option<&str>) -> u32 {
        let track_id = analysis.id;
        
        // Get or create artist ID
        let artist_id = self.get_or_create_artist(&analysis.artist);
        
        // Get or create album ID (associated with artist)
        let album_id = analysis.album.as_ref()
            .map(|a| self.get_or_create_album(a, artist_id))
            .unwrap_or(0);
        
        // Get or create genre ID  
        let genre_id = analysis.genre.as_ref()
            .map(|g| self.get_or_create_genre(g))
            .unwrap_or(0);
        
        // Get or create label ID (use empty string -> 0)
        let label_id = analysis.label.as_ref()
            .map(|l| self.get_or_create_label(l))
            .unwrap_or(0);
        
        // Get or create key ID
        let key_id = analysis.key
            .map(|k| self.get_or_create_key(k.to_rekordbox_id(), &k.name()))
            .unwrap_or(0);

        // Get or create artwork ID
        let artwork_id = artwork_path
            .map(|p| self.get_or_create_artwork(p))
            .unwrap_or(0);
        
        self.tracks.push(TrackInfo {
            analysis: analysis.clone(),
            artist_id,
            album_id,
            genre_id,
            label_id,
            key_id,
            artwork_id,
            analyze_path: analyze_path.to_string(),
        });
        
        track_id
    }
    
    /// Add a playlist
    pub fn add_playlist(&mut self, id: u32, parent_id: u32, name: &str, track_ids: Vec<u32>) {
        self.playlists.push(PlaylistInfo {
            id,
            parent_id,
            name: name.to_string(),
            is_folder: false,
            sort_order: self.playlists.len() as u32,
            track_ids,
        });
    }
    
    /// Add a playlist folder
    pub fn add_folder(&mut self, id: u32, parent_id: u32, name: &str) {
        self.playlists.push(PlaylistInfo {
            id,
            parent_id,
            name: name.to_string(),
            is_folder: true,
            sort_order: self.playlists.len() as u32,
            track_ids: Vec::new(),
        });
    }
    
    fn get_or_create_artist(&mut self, name: &str) -> u32 {
        if name.is_empty() {
            return 0;
        }
        if let Some(&id) = self.artists.get(name) {
            return id;
        }
        let id = self.next_artist_id;
        self.next_artist_id += 1;
        self.artists.insert(name.to_string(), id);
        id
    }
    
    fn get_or_create_album(&mut self, name: &str, artist_id: u32) -> u32 {
        if name.is_empty() {
            return 0;
        }
        let key = (name.to_string(), artist_id);
        if let Some(&id) = self.albums.get(&key) {
            return id;
        }
        let id = self.next_album_id;
        self.next_album_id += 1;
        self.albums.insert(key, id);
        id
    }
    
    fn get_or_create_genre(&mut self, name: &str) -> u32 {
        if name.is_empty() {
            return 0;
        }
        if let Some(&id) = self.genres.get(name) {
            return id;
        }
        let id = self.next_genre_id;
        self.next_genre_id += 1;
        self.genres.insert(name.to_string(), id);
        id
    }
    
    fn get_or_create_label(&mut self, name: &str) -> u32 {
        if name.is_empty() {
            return 0;
        }
        if let Some(&id) = self.labels.get(name) {
            return id;
        }
        let id = self.next_label_id;
        self.next_label_id += 1;
        self.labels.insert(name.to_string(), id);
        id
    }
    
    fn get_or_create_key(&mut self, rekordbox_id: u8, _name: &str) -> u32 {
        if let Some(&id) = self.keys.get(&rekordbox_id) {
            return id;
        }
        let id = self.next_key_id;
        self.next_key_id += 1;
        self.keys.insert(rekordbox_id, id);
        id
    }

    fn get_or_create_artwork(&mut self, path: &str) -> u32 {
        if path.is_empty() {
            return 0;
        }
        if let Some(&id) = self.artworks.get(path) {
            return id;
        }
        let id = self.next_artwork_id;
        self.next_artwork_id += 1;
        self.artworks.insert(path.to_string(), id);
        id
    }
    
    /// Build the complete PDB file
    /// 
    /// This creates a PDB file with all 20 required tables, each with:
    /// 1. An INDEX page (flags 0x64)
    /// 2. One or more DATA pages (flags 0x24/0x34)
    pub fn build(&self) -> Result<Vec<u8>> {
        let mut all_pages: Vec<Vec<u8>> = Vec::new();
        let mut header = FileHeader::new();
        
        // Reserve page 0 for header
        all_pages.push(vec![0u8; PAGE_SIZE]);
        let mut next_page_index = 1u32;
        
        // Global sequence counter - increments with each DATA page
        // Per REX: sequence starts at 2 and increments
        let mut sequence = 2u32;
        
        // Collect table info while building
        struct TableInfo {
            page_type: PageType,
            index_page_idx: u32,
            last_data_page: u32,
        }
        let mut table_infos: Vec<TableInfo> = Vec::new();
        
        // Build all 20 tables in order
        for page_type in PageType::all_types() {
            let (index_page, data_pages, index_page_idx, last_data_page, new_sequence) = 
                self.build_table_with_sequence(*page_type, &mut next_page_index, sequence)?;
            
            // Save info for later
            table_infos.push(TableInfo {
                page_type: *page_type,
                index_page_idx,
                last_data_page,
            });
            
            // Update sequence for next table
            sequence = new_sequence;
            
            // Add pages
            all_pages.push(index_page);
            all_pages.extend(data_pages);
        }
        
        let page_count = next_page_index; // first index past all real pages

        // empty_candidate scheme (matches golden; rekordbox's Page Manager
        // validates this on open):
        //   empty tables -> their reserved zeroed page (last_data_page + 1)
        //   data tables  -> a distinct phantom page beyond the file
        //   next_unused_page is strictly above every phantom
        //
        // CRITICAL: each data table's LAST data page must have its next_page
        // (0x0C) point to THAT table's own phantom empty_candidate — not the
        // global next_unused. Golden gives every table a distinct phantom and
        // terminates its page chain there; pointing them all at one page makes
        // the page-chain graph malformed and the Page Manager rejects the file
        // ("Page Manager Open returns ERROR").
        let mut phantom = page_count;
        // Remember, per table, the empty_candidate and the last data page to patch.
        let mut patch_targets: Vec<(u32 /*last_data_page*/, u32 /*empty_candidate*/)> = Vec::new();
        for info in &table_infos {
            let has_data = info.last_data_page > info.index_page_idx;
            let empty_candidate = if has_data {
                let p = phantom;
                phantom += 1;
                p
            } else {
                info.last_data_page + 1
            };
            if has_data {
                patch_targets.push((info.last_data_page, empty_candidate));
            }
            header.add_table(TablePointer::new(
                info.page_type,
                empty_candidate,
                info.index_page_idx,
                info.last_data_page,
            ));
        }
        let final_next_unused = phantom; // strictly above every phantom empty_candidate

        // Patch each data table's LAST data page: next_page (0x0C) -> that
        // table's phantom empty_candidate. Interior data pages already point to
        // their real successor from build_table_with_sequence and are left as-is.
        for (last_data_page, empty_candidate) in &patch_targets {
            let page = &mut all_pages[*last_data_page as usize];
            debug_assert!(
                matches!(page[0x1B], 0x24 | 0x34),
                "patch target is not a DATA page"
            );
            page[0x0C..0x10].copy_from_slice(&empty_candidate.to_le_bytes());
        }
        
        // Update header with final values
        header.next_unused_page = final_next_unused;
        header.sequence = sequence;
        header.track_count = self.tracks.len() as u32;  // Set track count (field 0x10)
        all_pages[0] = header.to_page();
        
        // Flatten to single buffer
        let mut output = Vec::with_capacity(all_pages.len() * PAGE_SIZE);
        for page in all_pages {
            output.extend_from_slice(&page);
        }
        
        Ok(output)
    }
    
    /// Build a single table with sequence tracking
    /// Returns: (index_page, data_pages, index_page_idx, last_data_page_idx, new_sequence)
    fn build_table_with_sequence(&self, page_type: PageType, next_idx: &mut u32, mut sequence: u32) -> Result<(Vec<u8>, Vec<Vec<u8>>, u32, u32, u32)> {
        let index_page_idx = *next_idx;
        *next_idx += 1;
        
        let data_page_idx = *next_idx;
        
        // Build data pages based on table type
        let (mut data_pages, has_data) = match page_type {
            PageType::Tracks => self.build_track_data_pages(next_idx)?,
            PageType::Genres => self.build_genre_data_pages(next_idx)?,
            PageType::Artists => self.build_artist_data_pages(next_idx)?,
            PageType::Albums => self.build_album_data_pages(next_idx)?,
            PageType::Labels => self.build_label_data_pages(next_idx)?,
            PageType::Keys => self.build_key_data_pages(next_idx)?,
            PageType::Colors => self.build_color_data_pages(next_idx)?,
            PageType::PlaylistTree => self.build_playlist_tree_data_pages(next_idx)?,
            PageType::PlaylistEntries => self.build_playlist_entry_data_pages(next_idx)?,
            PageType::HistoryPlaylists => self.build_history_playlist_data_pages(next_idx)?,
            PageType::Artwork => self.build_artwork_data_pages(next_idx)?,
            PageType::Columns => self.build_columns_data_pages(next_idx)?,
            PageType::Unknown17 => self.build_unknown17_data_pages(next_idx)?,
            PageType::Unknown18 => self.build_unknown18_data_pages(next_idx)?,
            PageType::History => self.build_history_data_pages(next_idx)?,
            _ => self.build_empty_data_pages(next_idx)?,
        };
        
        // Patch DATA pages with sequence counter
        // Note: next_page (0x0C) is patched later in build() to point to final next_unused_page
        for page in data_pages.iter_mut() {
            if page[0x1B] != 0 {  // Only patch non-empty pages
                // 0x10: sequence = global sequence counter
                page[0x10..0x14].copy_from_slice(&sequence.to_le_bytes());
                sequence += 1;
            }
        }
        
        // Extract num_row_offsets from last data page
        let num_row_offsets = if has_data && !data_pages.is_empty() {
            // Per Kaitai, num_rows_small is at 0x18
            data_pages.last().unwrap()[0x18] as u32
        } else {
            0
        };
        
        // Build index page
        let index_page = IndexPageBuilder::new(index_page_idx, page_type)
            .finalize(data_page_idx, has_data, num_row_offsets);
        
        // Calculate last_data_page
        let last_data_page = if has_data && !data_pages.is_empty() {
            data_page_idx + (data_pages.len() as u32) - 1
        } else {
            index_page_idx  // Empty tables: last == first
        };
        
        Ok((index_page, data_pages, index_page_idx, last_data_page, sequence))
    }
    
    /// Build empty data page (for tables with no content)
    /// Empty pages are completely zeros in rekordbox format
    fn build_empty_data_pages(&self, next_idx: &mut u32) -> Result<(Vec<Vec<u8>>, bool)> {
        *next_idx += 1;
        Ok((vec![PageBuilder::empty_page()], false))
    }
    
    /// Build track data pages
    fn build_track_data_pages(&self, next_idx: &mut u32) -> Result<(Vec<Vec<u8>>, bool)> {
        if self.tracks.is_empty() {
            return self.build_empty_data_pages(next_idx);
        }
        
        let mut pages: Vec<Vec<u8>> = Vec::new();
        let mut current_page = PageBuilder::new(*next_idx, PageType::Tracks);
        *next_idx += 1;
        
        for track in &self.tracks {
            let row_data = self.build_track_row(track)?;
            
            if current_page.would_overflow(row_data.len()) {
                let next = *next_idx;
                pages.push(current_page.finalize(next));
                current_page = PageBuilder::new(next, PageType::Tracks);
                *next_idx += 1;
            }
            
            current_page.write_row(&row_data)?;
        }
        
        pages.push(current_page.finalize(0xFFFFFFFF));
        Ok((pages, true))
    }
    
    /// Build genre data pages
    fn build_genre_data_pages(&self, next_idx: &mut u32) -> Result<(Vec<Vec<u8>>, bool)> {
        if self.genres.is_empty() {
            return self.build_empty_data_pages(next_idx);
        }
        
        let mut pages: Vec<Vec<u8>> = Vec::new();
        let mut current_page = PageBuilder::new(*next_idx, PageType::Genres);
        *next_idx += 1;
        
        let mut genres: Vec<_> = self.genres.iter().collect();
        genres.sort_by_key(|(_, &id)| id);
        
        for (name, &id) in genres {
            let row_data = self.build_genre_row(id, name);
            
            if current_page.would_overflow(row_data.len()) {
                let next = *next_idx;
                pages.push(current_page.finalize(next));
                current_page = PageBuilder::new(next, PageType::Genres);
                *next_idx += 1;
            }
            
            current_page.write_row(&row_data)?;
        }
        
        pages.push(current_page.finalize(0xFFFFFFFF));
        Ok((pages, true))
    }
    
    /// Build artist data pages
    fn build_artist_data_pages(&self, next_idx: &mut u32) -> Result<(Vec<Vec<u8>>, bool)> {
        if self.artists.is_empty() {
            return self.build_empty_data_pages(next_idx);
        }
        
        let mut pages: Vec<Vec<u8>> = Vec::new();
        let mut current_page = PageBuilder::new(*next_idx, PageType::Artists);
        *next_idx += 1;
        
        let mut artists: Vec<_> = self.artists.iter().collect();
        artists.sort_by_key(|(_, &id)| id);
        
        for (name, &id) in artists {
            let row_data = self.build_artist_row(id, name);
            
            if current_page.would_overflow(row_data.len()) {
                let next = *next_idx;
                pages.push(current_page.finalize(next));
                current_page = PageBuilder::new(next, PageType::Artists);
                *next_idx += 1;
            }
            
            current_page.write_row(&row_data)?;
        }
        
        pages.push(current_page.finalize(0xFFFFFFFF));
        Ok((pages, true))
    }
    
    /// Build album data pages
    fn build_album_data_pages(&self, next_idx: &mut u32) -> Result<(Vec<Vec<u8>>, bool)> {
        if self.albums.is_empty() {
            return self.build_empty_data_pages(next_idx);
        }
        
        let mut pages: Vec<Vec<u8>> = Vec::new();
        let mut current_page = PageBuilder::new(*next_idx, PageType::Albums);
        *next_idx += 1;
        
        let mut albums: Vec<_> = self.albums.iter().collect();
        albums.sort_by_key(|((_, _), &id)| id);
        
        for ((name, artist_id), &id) in albums {
            let row_data = self.build_album_row(id, *artist_id, name);
            
            if current_page.would_overflow(row_data.len()) {
                let next = *next_idx;
                pages.push(current_page.finalize(next));
                current_page = PageBuilder::new(next, PageType::Albums);
                *next_idx += 1;
            }
            
            current_page.write_row(&row_data)?;
        }
        
        pages.push(current_page.finalize(0xFFFFFFFF));
        Ok((pages, true))
    }
    
    /// Build label data pages
    fn build_label_data_pages(&self, next_idx: &mut u32) -> Result<(Vec<Vec<u8>>, bool)> {
        if self.labels.is_empty() {
            return self.build_empty_data_pages(next_idx);
        }
        
        let mut pages: Vec<Vec<u8>> = Vec::new();
        let mut current_page = PageBuilder::new(*next_idx, PageType::Labels);
        *next_idx += 1;
        
        let mut labels: Vec<_> = self.labels.iter().collect();
        labels.sort_by_key(|(_, &id)| id);
        
        for (name, &id) in labels {
            let row_data = self.build_label_row(id, name);
            
            if current_page.would_overflow(row_data.len()) {
                let next = *next_idx;
                pages.push(current_page.finalize(next));
                current_page = PageBuilder::new(next, PageType::Labels);
                *next_idx += 1;
            }
            
            current_page.write_row(&row_data)?;
        }
        
        pages.push(current_page.finalize(0xFFFFFFFF));
        Ok((pages, true))
    }
    
    /// Build key data pages
    fn build_key_data_pages(&self, next_idx: &mut u32) -> Result<(Vec<Vec<u8>>, bool)> {
        if self.keys.is_empty() {
            return self.build_empty_data_pages(next_idx);
        }
        
        let mut pages: Vec<Vec<u8>> = Vec::new();
        let mut current_page = PageBuilder::new(*next_idx, PageType::Keys);
        *next_idx += 1;
        
        let mut keys: Vec<_> = self.keys.iter().collect();
        keys.sort_by_key(|(_, &id)| id);
        
        for (&rekordbox_id, &id) in keys {
            let key = crate::track::Key::from_rekordbox_id(rekordbox_id);
            let row_data = self.build_key_row(id, &key.name());
            
            if current_page.would_overflow(row_data.len()) {
                let next = *next_idx;
                pages.push(current_page.finalize(next));
                current_page = PageBuilder::new(next, PageType::Keys);
                *next_idx += 1;
            }
            
            current_page.write_row(&row_data)?;
        }
        
        pages.push(current_page.finalize(0xFFFFFFFF));
        Ok((pages, true))
    }
    
    /// Build color data pages (always includes 8 default colors)
    fn build_color_data_pages(&self, next_idx: &mut u32) -> Result<(Vec<Vec<u8>>, bool)> {
        let mut pages: Vec<Vec<u8>> = Vec::new();
        let mut current_page = PageBuilder::new(*next_idx, PageType::Colors);
        *next_idx += 1;
        
        // Default colors from rekordbox (same as rex project)
        let colors = [
            (1, "Pink"),
            (2, "Red"),
            (3, "Orange"),
            (4, "Yellow"),
            (5, "Green"),
            (6, "Aqua"),
            (7, "Blue"),
            (8, "Purple"),
        ];
        
        for (id, name) in colors {
            let row_data = self.build_color_row(id, name);
            current_page.write_row(&row_data)?;
        }
        
        pages.push(current_page.finalize(0xFFFFFFFF));
        Ok((pages, true))
    }
    
    /// Build playlist tree data pages
    fn build_playlist_tree_data_pages(&self, next_idx: &mut u32) -> Result<(Vec<Vec<u8>>, bool)> {
        if self.playlists.is_empty() {
            return self.build_empty_data_pages(next_idx);
        }
        
        let mut pages: Vec<Vec<u8>> = Vec::new();
        let mut current_page = PageBuilder::new(*next_idx, PageType::PlaylistTree);
        *next_idx += 1;
        
        for playlist in &self.playlists {
            let row_data = self.build_playlist_tree_row(playlist);
            
            if current_page.would_overflow(row_data.len()) {
                let next = *next_idx;
                pages.push(current_page.finalize(next));
                current_page = PageBuilder::new(next, PageType::PlaylistTree);
                *next_idx += 1;
            }
            
            current_page.write_row(&row_data)?;
        }
        
        pages.push(current_page.finalize(0xFFFFFFFF));
        Ok((pages, true))
    }
    
    /// Build playlist entry data pages
    fn build_playlist_entry_data_pages(&self, next_idx: &mut u32) -> Result<(Vec<Vec<u8>>, bool)> {
        let entries: Vec<_> = self.playlists.iter()
            .filter(|p| !p.is_folder)
            .flat_map(|p| {
                p.track_ids.iter().enumerate().map(move |(idx, &track_id)| {
                    (idx as u32 + 1, track_id, p.id)
                })
            })
            .collect();
        
        if entries.is_empty() {
            return self.build_empty_data_pages(next_idx);
        }
        
        let mut pages: Vec<Vec<u8>> = Vec::new();
        let mut current_page = PageBuilder::new(*next_idx, PageType::PlaylistEntries);
        *next_idx += 1;
        
        for (entry_index, track_id, playlist_id) in entries {
            let row_data = self.build_playlist_entry_row(entry_index, track_id, playlist_id);
            
            if current_page.would_overflow(row_data.len()) {
                let next = *next_idx;
                pages.push(current_page.finalize(next));
                current_page = PageBuilder::new(next, PageType::PlaylistEntries);
                *next_idx += 1;
            }
            
            current_page.write_row(&row_data)?;
        }
        
        pages.push(current_page.finalize(0xFFFFFFFF));
        Ok((pages, true))
    }
    
    /// Build history playlist data pages
    fn build_history_playlist_data_pages(&self, next_idx: &mut u32) -> Result<(Vec<Vec<u8>>, bool)> {
        // For now, just create empty table
        self.build_empty_data_pages(next_idx)
    }
    
    /// Build artwork data pages
    fn build_artwork_data_pages(&self, next_idx: &mut u32) -> Result<(Vec<Vec<u8>>, bool)> {
        if self.artworks.is_empty() {
            return self.build_empty_data_pages(next_idx);
        }
        
        let mut pages: Vec<Vec<u8>> = Vec::new();
        let mut current_page = PageBuilder::new(*next_idx, PageType::Artwork);
        *next_idx += 1;
        
        let mut artworks: Vec<_> = self.artworks.iter().collect();
        artworks.sort_by_key(|(_, &id)| id);
        
        for (path, &id) in artworks {
            let row_data = self.build_artwork_row(id, path);
            
            if current_page.would_overflow(row_data.len()) {
                let next = *next_idx;
                pages.push(current_page.finalize(next));
                current_page = PageBuilder::new(next, PageType::Artwork);
                *next_idx += 1;
            }
            
            current_page.write_row(&row_data)?;
        }
        
        pages.push(current_page.finalize(0xFFFFFFFF));
        Ok((pages, true))
    }
    
    /// Build columns data pages (type 16)
    /// Contains column name metadata required by rekordbox
    fn build_columns_data_pages(&self, next_idx: &mut u32) -> Result<(Vec<Vec<u8>>, bool)> {
        let mut pages: Vec<Vec<u8>> = Vec::new();
        let mut current_page = PageBuilder::new(*next_idx, PageType::Columns);
        *next_idx += 1;
        
        // Column metadata extracted from rekordbox 6.8 export
        // Format: id(u2) + subtype(u2) + name(DeviceSQL UTF-16LE string)
        // The subtype appears to be 0x80 + column_id
        let columns_data: &[&[u8]] = &[
            &[1, 0, 128, 0, 144, 18, 0, 0, 250, 255, 71, 0, 69, 0, 78, 0, 82, 0, 69, 0, 251, 255, 0, 0],  // GENRE
            &[2, 0, 129, 0, 144, 20, 0, 0, 250, 255, 65, 0, 82, 0, 84, 0, 73, 0, 83, 0, 84, 0, 251, 255],  // ARTIST
            &[3, 0, 130, 0, 144, 18, 0, 0, 250, 255, 65, 0, 76, 0, 66, 0, 85, 0, 77, 0, 251, 255, 0, 0],  // ALBUM
            &[4, 0, 131, 0, 144, 18, 0, 0, 250, 255, 84, 0, 82, 0, 65, 0, 67, 0, 75, 0, 251, 255, 0, 0],  // TRACK
            &[5, 0, 133, 0, 144, 14, 0, 0, 250, 255, 66, 0, 80, 0, 77, 0, 251, 255, 0, 0],  // BPM
            &[6, 0, 134, 0, 144, 20, 0, 0, 250, 255, 82, 0, 65, 0, 84, 0, 73, 0, 78, 0, 71, 0, 251, 255],  // RATING
            &[7, 0, 135, 0, 144, 16, 0, 0, 250, 255, 89, 0, 69, 0, 65, 0, 82, 0, 251, 255],  // YEAR
            &[8, 0, 136, 0, 144, 22, 0, 0, 250, 255, 82, 0, 69, 0, 77, 0, 73, 0, 88, 0, 69, 0, 82, 0, 251, 255, 0, 0],  // REMIXER
            &[9, 0, 137, 0, 144, 18, 0, 0, 250, 255, 76, 0, 65, 0, 66, 0, 69, 0, 76, 0, 251, 255, 0, 0],  // LABEL
            &[10, 0, 138, 0, 144, 38, 0, 0, 250, 255, 79, 0, 82, 0, 73, 0, 71, 0, 73, 0, 78, 0, 65, 0, 76, 0, 32, 0, 65, 0, 82, 0, 84, 0, 73, 0, 83, 0, 84, 0, 251, 255, 0, 0],  // ORIGINAL ARTIST
            &[11, 0, 139, 0, 144, 14, 0, 0, 250, 255, 75, 0, 69, 0, 89, 0, 251, 255, 0, 0],  // KEY
            &[12, 0, 141, 0, 144, 14, 0, 0, 250, 255, 67, 0, 85, 0, 69, 0, 251, 255, 0, 0],  // CUE
            &[13, 0, 142, 0, 144, 18, 0, 0, 250, 255, 67, 0, 79, 0, 76, 0, 79, 0, 82, 0, 251, 255, 0, 0],  // COLOR
            &[14, 0, 146, 0, 144, 16, 0, 0, 250, 255, 84, 0, 73, 0, 77, 0, 69, 0, 251, 255],  // TIME
            &[15, 0, 147, 0, 144, 22, 0, 0, 250, 255, 66, 0, 73, 0, 84, 0, 82, 0, 65, 0, 84, 0, 69, 0, 251, 255, 0, 0],  // BITRATE
            &[16, 0, 148, 0, 144, 26, 0, 0, 250, 255, 70, 0, 73, 0, 76, 0, 69, 0, 32, 0, 78, 0, 65, 0, 77, 0, 69, 0, 251, 255, 0, 0],  // FILE NAME
            &[17, 0, 132, 0, 144, 24, 0, 0, 250, 255, 80, 0, 76, 0, 65, 0, 89, 0, 76, 0, 73, 0, 83, 0, 84, 0, 251, 255],  // PLAYLIST
            &[18, 0, 152, 0, 144, 32, 0, 0, 250, 255, 72, 0, 79, 0, 84, 0, 32, 0, 67, 0, 85, 0, 69, 0, 32, 0, 66, 0, 65, 0, 78, 0, 75, 0, 251, 255],  // HOT CUE BANK
            &[19, 0, 149, 0, 144, 22, 0, 0, 250, 255, 72, 0, 73, 0, 83, 0, 84, 0, 79, 0, 82, 0, 89, 0, 251, 255, 0, 0],  // HISTORY
            &[20, 0, 145, 0, 144, 20, 0, 0, 250, 255, 83, 0, 69, 0, 65, 0, 82, 0, 67, 0, 72, 0, 251, 255],  // SEARCH
            &[21, 0, 150, 0, 144, 24, 0, 0, 250, 255, 67, 0, 79, 0, 77, 0, 77, 0, 69, 0, 78, 0, 84, 0, 83, 0, 251, 255],  // COMMENTS
            &[22, 0, 140, 0, 144, 28, 0, 0, 250, 255, 68, 0, 65, 0, 84, 0, 69, 0, 32, 0, 65, 0, 68, 0, 68, 0, 69, 0, 68, 0, 251, 255],  // DATE ADDED
            &[23, 0, 151, 0, 144, 34, 0, 0, 250, 255, 68, 0, 74, 0, 32, 0, 80, 0, 76, 0, 65, 0, 89, 0, 32, 0, 67, 0, 79, 0, 85, 0, 78, 0, 84, 0, 251, 255, 0, 0],  // DJ PLAY COUNT
            &[24, 0, 144, 0, 144, 20, 0, 0, 250, 255, 70, 0, 79, 0, 76, 0, 68, 0, 69, 0, 82, 0, 251, 255],  // FOLDER
            &[25, 0, 161, 0, 144, 22, 0, 0, 250, 255, 68, 0, 69, 0, 70, 0, 65, 0, 85, 0, 76, 0, 84, 0, 251, 255, 0, 0],  // DEFAULT
            &[26, 0, 162, 0, 144, 24, 0, 0, 250, 255, 65, 0, 76, 0, 80, 0, 72, 0, 65, 0, 66, 0, 69, 0, 84, 0, 251, 255],  // ALPHABET
            &[27, 0, 170, 0, 144, 24, 0, 0, 250, 255, 77, 0, 65, 0, 84, 0, 67, 0, 72, 0, 73, 0, 78, 0, 71, 0, 251, 255],  // MATCHING
        ];
        
        for row in columns_data {
            current_page.write_row(row)?;
        }
        
        pages.push(current_page.finalize(0xFFFFFFFF));
        Ok((pages, true))
    }
    
    /// Build unknown17 data pages (type 17, uk17 in Kaitai spec)
    /// Kaitai spec defines: 4 x u4 = 16 bytes per row
    fn build_unknown17_data_pages(&self, next_idx: &mut u32) -> Result<(Vec<Vec<u8>>, bool)> {
        let mut pages: Vec<Vec<u8>> = Vec::new();
        let mut current_page = PageBuilder::new(*next_idx, PageType::Unknown17);
        *next_idx += 1;
        
        // Static dataset from rekordbox binary analysis
        // ACTUAL format: 4 x u16 = 8 bytes per row (NOT u32!)
        let dataset: &[(u16, u16, u16, u16)] = &[
            (0x1, 0x1, 0x163, 0x0),
            (0x5, 0x6, 0x105, 0x0),
            (0x6, 0x7, 0x163, 0x0),
            (0x7, 0x8, 0x163, 0x0),
            (0x8, 0x9, 0x163, 0x0),
            (0x9, 0xa, 0x163, 0x0),
            (0xa, 0xb, 0x163, 0x0),
            (0xd, 0xf, 0x163, 0x0),
            (0xe, 0x13, 0x104, 0x0),
            (0xf, 0x14, 0x106, 0x0),
            (0x10, 0x15, 0x163, 0x0),
            (0x12, 0x17, 0x163, 0x0),
            (0x2, 0x2, 0x2, 0x1),
            (0x3, 0x3, 0x3, 0x2),
            (0x4, 0x4, 0x1, 0x3),
            (0xb, 0xc, 0x63, 0x4),
            (0x11, 0x5, 0x63, 0x5),
            (0x13, 0x16, 0x63, 0x6),
            (0x14, 0x12, 0x63, 0x7),
            (0x1b, 0x1a, 0x263, 0x8),
            (0x18, 0x11, 0x63, 0x9),
            (0x16, 0x1b, 0x63, 0xa),
        ];
        
        for &(u1, u2, u3, u4) in dataset {
            let mut row = Vec::with_capacity(8);
            row.extend_from_slice(&u1.to_le_bytes());
            row.extend_from_slice(&u2.to_le_bytes());
            row.extend_from_slice(&u3.to_le_bytes());
            row.extend_from_slice(&u4.to_le_bytes());
            current_page.write_row(&row)?;
        }
        
        pages.push(current_page.finalize(0xFFFFFFFF));
        Ok((pages, true))
    }
    
    /// Build unknown18 data pages (type 18)
    /// Contains mapping/configuration data required by rekordbox
    /// Format: 4 x u16 = 8 bytes per row
    fn build_unknown18_data_pages(&self, next_idx: &mut u32) -> Result<(Vec<Vec<u8>>, bool)> {
        let mut pages: Vec<Vec<u8>> = Vec::new();
        let mut current_page = PageBuilder::new(*next_idx, PageType::Unknown18);
        *next_idx += 1;
        
        // System mapping data extracted from rekordbox 6.8 export
        let unknown18_data: &[(u16, u16, u16, u16)] = &[
            (1, 6, 1, 0),
            (21, 7, 1, 0),
            (14, 8, 1, 0),
            (8, 9, 1, 0),
            (9, 10, 1, 0),
            (10, 11, 1, 0),
            (15, 13, 1, 0),
            (13, 15, 1, 0),
            (23, 16, 1, 0),
            (22, 17, 1, 0),
            (25, 0, 256, 0),
            (26, 1, 512, 0),
            (2, 2, 768, 0),
            (3, 3, 1024, 0),
            (5, 4, 1280, 0),
            (6, 5, 1536, 0),
            (11, 12, 1792, 0),
        ];
        
        for &(u1, u2, u3, u4) in unknown18_data {
            let mut row = Vec::with_capacity(8);
            row.extend_from_slice(&u1.to_le_bytes());
            row.extend_from_slice(&u2.to_le_bytes());
            row.extend_from_slice(&u3.to_le_bytes());
            row.extend_from_slice(&u4.to_le_bytes());
            current_page.write_row(&row)?;
        }
        
        pages.push(current_page.finalize(0xFFFFFFFF));
        Ok((pages, true))
    }
    
    /// Build history data pages (type 19)
    /// Contains sync metadata required by rekordbox
    fn build_history_data_pages(&self, next_idx: &mut u32) -> Result<(Vec<Vec<u8>>, bool)> {
        let mut pages: Vec<Vec<u8>> = Vec::new();
        let mut current_page = PageBuilder::new(*next_idx, PageType::History);
        *next_idx += 1;
        
        // History/sync metadata row extracted from rekordbox 6.8 export
        // 40 bytes total:
        // [0-1]: 0x0280 (subtype/flags, little-endian)
        // [2-11]: zeros (10 bytes padding)
        // [12]: 0x17 (23 = string section length)
        // [13-22]: "2025-01-01" (date in ASCII, 10 bytes)
        // [23-24]: 0x191E (additional data, little-endian)
        // [25]: 0x0B (11 = marker/length)
        // [26-29]: "1000" (version string in ASCII)
        // [30]: 0x03 (DeviceSQL string marker)
        // [31-39]: zeros (9 bytes padding)
        let history_row: [u8; 40] = [
            0x80, 0x02,  // subtype
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // padding
            0x17,  // string section length (23)
            b'2', b'0', b'2', b'5', b'-', b'0', b'1', b'-', b'0', b'1',  // date "2025-01-01"
            0x19, 0x1E,  // additional timestamp data
            0x0B,  // marker (11)
            b'1', b'0', b'0', b'0',  // version "1000"
            0x03,  // DeviceSQL marker
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // padding
        ];
        
        current_page.write_row(&history_row)?;
        
        pages.push(current_page.finalize(0xFFFFFFFF));
        Ok((pages, true))
    }
    
    /// Build a single track row
    fn build_track_row(&self, track: &TrackInfo) -> Result<Vec<u8>> {
        let analysis = &track.analysis;
        
        // Track row has fixed fields + 21 string offsets
        // We need to calculate the total size first to determine string offsets
        
        // Fixed part: 0x5A bytes (90 bytes) before string offsets
        // Then 21 × 2-byte offsets = 42 bytes
        // Total fixed header: 132 bytes
        const FIXED_SIZE: usize = 0x5E; // was 0x5A; +4 for unknown3/unknown4 (see below)
        const STRING_COUNT: usize = 21;
        const HEADER_SIZE: usize = FIXED_SIZE + STRING_COUNT * 2;
        
        // Build all strings
        let strings: Vec<Vec<u8>> = vec![
            encode_isrc(""), // 0: ISRC
            encode_string(""), // 1: lyricist
            encode_string(""), // 2: unknown (version?)
            encode_string(""), // 3: unknown
            encode_string(""), // 4: unknown
            encode_string(""), // 5: message
            encode_string(""), // 6: publish_track_info
            encode_string(""), // 7: autoload_hotcues
            encode_string(""), // 8: unknown
            encode_string(""), // 9: unknown
            encode_string(""), // 10: date_added
            encode_string(analysis.year.map(|y| format!("{}-01-01", y)).as_deref().unwrap_or("")), // 11: release_date
            encode_string(""), // 12: mix_name
            encode_string(""), // 13: unknown
            encode_string(&format!("/{}", track.analyze_path.trim_start_matches('/'))), // 14: analyze_path (must have leading /)
            encode_string(""), // 15: analyze_date
            encode_string(analysis.comment.as_deref().unwrap_or("")), // 16: comment
            encode_string(&analysis.title), // 17: title
            encode_string(""), // 18: unknown
            encode_string(&analysis.file_path.split('/').last().unwrap_or(&analysis.file_path)), // 19: filename
            encode_string(&analysis.file_path), // 20: file_path
        ];
        
        // Calculate offsets (relative to row start)
        let mut string_offsets = Vec::with_capacity(STRING_COUNT);
        let mut current_offset = HEADER_SIZE;
        for s in &strings {
            string_offsets.push(current_offset as u16);
            current_offset += s.len();
        }
        
        // Build the row
        let mut row = Vec::with_capacity(current_offset);
        
        // Fixed fields (0x00 - 0x5D)
        // 0x00-0x01: subtype (0x0024 for track with 2-byte offsets)
        row.extend_from_slice(&SUBTYPE_TRACK.to_le_bytes());
        
        // 0x02-0x03: index_shift (0x20 = 32 means 2-byte string offsets)
        // Track rows MUST use 2-byte offsets since strings can be >255 bytes from row start
        row.extend_from_slice(&0x20u16.to_le_bytes());
        
        // 0x04-0x07: bitmask (controls string field presence)
        // Value 0x000C0700 is standard for rekordbox 6.x tracks
        row.extend_from_slice(&0x000C0700u32.to_le_bytes());
        
        // 0x08-0x0B: sample_rate
        row.extend_from_slice(&analysis.sample_rate.to_le_bytes());
        
        // 0x0C-0x0F: composer_id
        row.extend_from_slice(&0u32.to_le_bytes());
        
        // 0x10-0x13: file_size
        row.extend_from_slice(&(analysis.file_size as u32).to_le_bytes());
        
        // 0x14-0x17: unknown2
        row.extend_from_slice(&0u32.to_le_bytes());
        
        // 0x18-0x19: unknown3, 0x1A-0x1B: unknown4
        // REQUIRED. These two u2 fields were missing, which placed artwork_id and
        // every field after it 4 bytes too early relative to a real rekordbox row
        // (per the Crate Digger KSY track_row). Values are unknown-purpose; 0 is fine.
        row.extend_from_slice(&0u16.to_le_bytes());
        row.extend_from_slice(&0u16.to_le_bytes());
        
        // 0x1C-0x1F: artwork_id  (NOTE: offsets in the comments below are now +4)
        row.extend_from_slice(&track.artwork_id.to_le_bytes());
        
        // 0x1C-0x1F: key_id
        row.extend_from_slice(&track.key_id.to_le_bytes());
        
        // 0x20-0x23: original_artist_id
        row.extend_from_slice(&0u32.to_le_bytes());
        
        // 0x24-0x27: label_id
        row.extend_from_slice(&track.label_id.to_le_bytes());
        
        // 0x28-0x2B: remixer_id
        row.extend_from_slice(&0u32.to_le_bytes());
        
        // 0x2C-0x2F: bitrate (in kbps)
        row.extend_from_slice(&analysis.bitrate.to_le_bytes());
        
        // 0x30-0x33: track_number
        row.extend_from_slice(&analysis.track_number.unwrap_or(0).to_le_bytes());
        
        // 0x34-0x37: tempo (BPM × 100)
        let tempo = (analysis.bpm * 100.0) as u32;
        row.extend_from_slice(&tempo.to_le_bytes());
        
        // 0x38-0x3B: genre_id
        row.extend_from_slice(&track.genre_id.to_le_bytes());
        
        // 0x3C-0x3F: album_id
        row.extend_from_slice(&track.album_id.to_le_bytes());
        
        // 0x40-0x43: artist_id
        row.extend_from_slice(&track.artist_id.to_le_bytes());
        
        // 0x44-0x47: id
        row.extend_from_slice(&analysis.id.to_le_bytes());
        
        // 0x48-0x49: disc_number
        row.extend_from_slice(&1u16.to_le_bytes());
        
        // 0x4A-0x4B: play_count
        row.extend_from_slice(&0u16.to_le_bytes());
        
        // 0x4C-0x4D: year
        row.extend_from_slice(&analysis.year.unwrap_or(0).to_le_bytes());
        
        // 0x4E-0x4F: sample_depth
        row.extend_from_slice(&analysis.bit_depth.to_le_bytes());
        
        // 0x50-0x51: duration (seconds)
        row.extend_from_slice(&(analysis.duration_secs as u16).to_le_bytes());
        
        // 0x52-0x53: unknown - Kaitai says "always 41?"
        row.extend_from_slice(&41u16.to_le_bytes());
        
        // 0x54: color_id
        row.push(0);
        
        // 0x55: rating
        row.push(0);
        
        // 0x56-0x57: unknown - Kaitai says "always 1?"
        row.extend_from_slice(&1u16.to_le_bytes());
        
        // 0x58-0x59: unknown - Kaitai says "alternating 2 or 3"
        row.extend_from_slice(&0x0003u16.to_le_bytes());
        
        // 0x5A onwards: string offsets (21 × 2 bytes)
        for offset in &string_offsets {
            row.extend_from_slice(&offset.to_le_bytes());
        }
        
        // Append string data
        for s in &strings {
            row.extend_from_slice(s);
        }
        
        Ok(row)
    }
    
    /// Build a single artist row
    /// Kaitai spec: subtype(u2) + index_shift(u2) + id(u4) + 0x03(u1) + ofs_name_near(u1)
    /// For far (0x64): ofs_name_far(u2) at offset 0x0A
    fn build_artist_row(&self, id: u32, name: &str) -> Vec<u8> {
        let name_encoded = encode_string(name);
        let name_len = name_encoded.len();
        
        // Use near (1-byte) or far (2-byte) offset based on row size
        let use_near = name_len <= 200;
        
        let mut row = Vec::new();
        
        if use_near {
            // subtype: 0x0060
            row.extend_from_slice(&SUBTYPE_NEAR.to_le_bytes());
            // index_shift
            row.extend_from_slice(&0u16.to_le_bytes());
            // id
            row.extend_from_slice(&id.to_le_bytes());
            // 0x03 marker (u1)
            row.push(0x03);
            // ofs_name_near (u1): header is 10 bytes (2+2+4+1+1), so name at offset 10
            row.push(10);
        } else {
            // subtype: 0x0064
            row.extend_from_slice(&SUBTYPE_FAR.to_le_bytes());
            // index_shift
            row.extend_from_slice(&0u16.to_le_bytes());
            // id
            row.extend_from_slice(&id.to_le_bytes());
            // 0x03 marker (u1) - still required per Kaitai spec
            row.push(0x03);
            // ofs_name_near (u1) - unused but present
            row.push(0);
            // ofs_name_far (u2) at offset 0x0A: header is 12 bytes (2+2+4+1+1+2)
            row.extend_from_slice(&12u16.to_le_bytes());
        }
        
        // Append name string
        row.extend_from_slice(&name_encoded);
        
        row
    }
    
    /// Build a single album row
    /// Kaitai spec: magic(u2) + index_shift(u2) + unknown(u4) + artist_id(u4) + 
    ///              id(u4) + unknown(u4) + 0x03(u1) + ofs_name(u1)
    /// Note: Kaitai only defines near format (0x80), far format (0x84) follows artist pattern
    fn build_album_row(&self, id: u32, artist_id: u32, name: &str) -> Vec<u8> {
        let name_encoded = encode_string(name);
        let name_len = name_encoded.len();
        
        let use_near = name_len <= 200;
        
        let mut row = Vec::new();
        
        if use_near {
            // subtype: 0x0080
            row.extend_from_slice(&0x0080u16.to_le_bytes());
            // index_shift
            row.extend_from_slice(&0u16.to_le_bytes());
            // unknown2 (4 bytes)
            row.extend_from_slice(&0u32.to_le_bytes());
            // artist_id
            row.extend_from_slice(&artist_id.to_le_bytes());
            // id
            row.extend_from_slice(&id.to_le_bytes());
            // unknown3 (4 bytes)
            row.extend_from_slice(&0u32.to_le_bytes());
            // 0x03 marker (u1)
            row.push(0x03);
            // ofs_name (u1): header is 22 bytes (2+2+4+4+4+4+1+1), name at 22
            row.push(22);
        } else {
            // subtype: 0x0084 (far format, follows artist_row pattern)
            row.extend_from_slice(&0x0084u16.to_le_bytes());
            // index_shift
            row.extend_from_slice(&0u16.to_le_bytes());
            // unknown2
            row.extend_from_slice(&0u32.to_le_bytes());
            // artist_id
            row.extend_from_slice(&artist_id.to_le_bytes());
            // id
            row.extend_from_slice(&id.to_le_bytes());
            // unknown3
            row.extend_from_slice(&0u32.to_le_bytes());
            // 0x03 marker (u1) - consistent with near format
            row.push(0x03);
            // ofs_name_near (u1) - unused but present for consistency
            row.push(0);
            // ofs_name_far (u2): header is 24 bytes (2+2+4+4+4+4+1+1+2)
            row.extend_from_slice(&24u16.to_le_bytes());
        }
        
        row.extend_from_slice(&name_encoded);
        
        row
    }
    
    /// Build a single genre row
    /// Structure: id (4 bytes) + name (DeviceSQL string)
    fn build_genre_row(&self, id: u32, name: &str) -> Vec<u8> {
        let mut row = Vec::new();
        row.extend_from_slice(&id.to_le_bytes());
        row.extend_from_slice(&encode_string(name));
        row
    }
    
    /// Build a single key row
    /// Structure: id (4 bytes) + id2 (4 bytes) + name (DeviceSQL string)
    fn build_key_row(&self, id: u32, name: &str) -> Vec<u8> {
        let mut row = Vec::new();
        row.extend_from_slice(&id.to_le_bytes());
        row.extend_from_slice(&id.to_le_bytes()); // id2 is same as id
        row.extend_from_slice(&encode_string(name));
        row
    }
    
    /// Build a single label row
    /// Labels use the same format as genres: id (4 bytes) + name (DeviceSQL string)
    fn build_label_row(&self, id: u32, name: &str) -> Vec<u8> {
        let mut row = Vec::new();
        row.extend_from_slice(&id.to_le_bytes());
        row.extend_from_slice(&encode_string(name));
        row
    }
    
    /// Build a single color row
    /// Structure per Deep Symmetry docs:
    /// - bytes 0x00-0x04: unknown1 (5 bytes, zeros)
    /// - bytes 0x05-0x06: id (2 bytes)
    /// - byte 0x07: u3 (1 byte, zero)
    /// - bytes 0x08+: name (DeviceSQL string)
    fn build_color_row(&self, id: u32, name: &str) -> Vec<u8> {
        let mut row = Vec::new();
        // Actual structure from rekordbox binary analysis:
        // - bytes 0-3: zeros (4 bytes)
        // - byte 4: u2 = color id (MUST equal byte 5)
        // - byte 5: id = color id
        // - bytes 6-7: zeros (2 bytes)
        // - bytes 8+: name (DeviceSQL string)
        row.extend_from_slice(&[0u8; 4]);  // 4 zeros
        row.push(id as u8);                 // byte 4: u2 = id
        row.push(id as u8);                 // byte 5: id
        row.extend_from_slice(&[0u8; 2]);  // 2 zeros
        row.extend_from_slice(&encode_string(name));
        row
    }
    
    /// Build a single playlist tree row
    fn build_playlist_tree_row(&self, playlist: &PlaylistInfo) -> Vec<u8> {
        let name_encoded = encode_string(&playlist.name);
        
        let mut row = Vec::new();
        
        // parent_id (4 bytes)
        row.extend_from_slice(&playlist.parent_id.to_le_bytes());
        
        // unknown (4 bytes)
        row.extend_from_slice(&0u32.to_le_bytes());
        
        // sort_order (4 bytes)
        row.extend_from_slice(&playlist.sort_order.to_le_bytes());
        
        // id (4 bytes)
        row.extend_from_slice(&playlist.id.to_le_bytes());
        
        // raw_is_folder (4 bytes)
        row.extend_from_slice(&(if playlist.is_folder { 1u32 } else { 0u32 }).to_le_bytes());
        
        // name (DeviceSQL string)
        row.extend_from_slice(&name_encoded);
        
        row
    }
    
    /// Build a single playlist entry row
    fn build_playlist_entry_row(&self, entry_index: u32, track_id: u32, playlist_id: u32) -> Vec<u8> {
        let mut row = Vec::new();
        row.extend_from_slice(&entry_index.to_le_bytes());
        row.extend_from_slice(&track_id.to_le_bytes());
        row.extend_from_slice(&playlist_id.to_le_bytes());
        row
    }

    /// Build a single artwork row
    /// Structure: id (4 bytes) + path (DeviceSQL string)
    fn build_artwork_row(&self, id: u32, path: &str) -> Vec<u8> {
        let mut row = Vec::new();
        row.extend_from_slice(&id.to_le_bytes());
        row.extend_from_slice(&encode_string(path));
        row
    }
}

impl Default for PdbBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::*;
    
    fn make_test_track(id: u32, title: &str, artist: &str) -> TrackAnalysis {
        TrackAnalysis {
            id,
            file_path: format!("Contents/{}.mp3", title),
            title: title.to_string(),
            artist: artist.to_string(),
            album: Some("Test Album".to_string()),
            genre: Some("Electronic".to_string()),
            label: None,
            duration_secs: 180.0,
            sample_rate: 44100,
            bit_depth: 16,
            bitrate: 320,
            bpm: 128.0,
            key: Some(Key::new(9, false)), // Am
            beat_grid: BeatGrid::default(),
            waveform: Waveform::default(),
            cue_points: Vec::new(),
            file_size: 5_000_000,
            file_hash: 0x12345678,
            year: Some(2024),
            comment: None,
            track_number: Some(1),
            file_type: FileType::Mp3,
        }
    }
    
    #[test]
    fn test_pdb_builder_basic() {
        let mut builder = PdbBuilder::new();
        
        let track = make_test_track(1, "Test Track", "Test Artist");
        builder.add_track(&track, "PIONEER/USBANLZ/P000/00000001/ANLZ0000.DAT");
        
        let data = builder.build().unwrap();
        
        // Should be at least header + one track page
        assert!(data.len() >= PAGE_SIZE * 2);
        assert_eq!(data.len() % PAGE_SIZE, 0);
        
        // Check header magic (page size at offset 4)
        let page_size = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        assert_eq!(page_size, PAGE_SIZE as u32);
    }
    
    #[test]
    fn test_pdb_with_playlists() {
        let mut builder = PdbBuilder::new();
        
        let track1 = make_test_track(1, "Track 1", "Artist A");
        let track2 = make_test_track(2, "Track 2", "Artist B");
        
        builder.add_track(&track1, "PIONEER/USBANLZ/P000/00000001/ANLZ0000.DAT");
        builder.add_track(&track2, "PIONEER/USBANLZ/P000/00000002/ANLZ0000.DAT");
        
        builder.add_playlist(1, 0, "My Playlist", vec![1, 2]);
        
        let data = builder.build().unwrap();
        assert!(data.len() >= PAGE_SIZE * 2);
    }
}