// SPDX-License-Identifier: GPL-3.0-or-later
use super::{config::Layout, state::RoomBook};
use std::{fs, io::Write, path::PathBuf, time::Instant};

pub struct Store {
    path: PathBuf,
}
impl Store {
    pub fn open(path: PathBuf, layout: &Layout) -> Result<(Self, RoomBook), &'static str> {
        let book = match fs::read(&path) {
            Ok(bytes) => RoomBook::parse(&bytes, layout, Instant::now())?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => RoomBook::empty(layout.clone()),
            Err(_) => return Err("Cannot read voice state file"),
        };
        let store = Self { path };
        store.save(&book)?;
        Ok((store, book))
    }
    pub fn save(&self, book: &RoomBook) -> Result<(), &'static str> {
        let bytes =
            serde_json::to_vec_pretty(&book.document()).map_err(|_| "Cannot encode voice state")?;
        let tmp = self.path.with_extension("json.tmp");
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp).map_err(|_| "Cannot write voice state")?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| "Cannot persist voice state")?;
        fs::rename(&tmp, &self.path).map_err(|_| "Cannot replace voice state")
    }
}
