//! Heart-rate history in `%LOCALAPPDATA%\NTPods\heart-rate.sqlite3`.
//! A writer thread keeps disk I/O off the AAP receiver. Failure to open the database
//! disables history without stopping live readings. Session end times remain NULL after a crash.

use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};

use crate::hr::HrSample;

const SCHEMA_VERSION: i32 = 1;

enum Event {
    Sample {
        device: u64,
        ts_ms: i64,
        bpm: u16,
        confidence: u8,
    },
    EndSession {
        ts_ms: i64,
    },
}

/// Cloneable handle to the database writer.
#[derive(Clone)]
pub struct HrDb {
    tx: Option<Sender<Event>>,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl HrDb {
    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self { tx: None }
    }

    /// Open the database and start its writer; log errors without disabling the sensor.
    pub fn open(log: fn(&str)) -> HrDb {
        let path = match std::env::var("LOCALAPPDATA") {
            Ok(la) => std::path::Path::new(&la)
                .join("NTPods")
                .join("heart-rate.sqlite3"),
            Err(_) => {
                log("hrdb: LOCALAPPDATA not set — heart-rate tracking disabled");
                return HrDb { tx: None };
            }
        };
        let conn = match Connection::open(&path).and_then(|c| init(&c).map(|_| c)) {
            Ok(c) => c,
            Err(e) => {
                log(&format!(
                    "hrdb: cannot open {} ({e}) — heart-rate tracking disabled",
                    path.display()
                ));
                return HrDb { tx: None };
            }
        };
        log(&format!("hrdb: tracking heart rate in {}", path.display()));
        let (tx, rx) = mpsc::channel::<Event>();
        thread::spawn(move || writer(conn, rx, log));
        HrDb { tx: Some(tx) }
    }

    /// Record one validated reading from the AirPods at `device` (their address).
    pub fn sample(&self, device: u64, s: HrSample) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Event::Sample {
                device,
                ts_ms: now_ms(),
                bpm: s.bpm,
                confidence: s.confidence,
            });
        }
    }

    /// Close the open session, if any (HR switched off, or the buds went away).
    pub fn end_session(&self) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Event::EndSession { ts_ms: now_ms() });
        }
    }
}

fn init(c: &Connection) -> rusqlite::Result<()> {
    c.pragma_update(None, "journal_mode", "WAL")?;
    c.pragma_update(None, "synchronous", "NORMAL")?;
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS sessions (
             id          INTEGER PRIMARY KEY,
             device      TEXT    NOT NULL,          -- AirPods address, aa:bb:cc:dd:ee:ff
             started_at  INTEGER NOT NULL,          -- Unix ms of the first reading
             ended_at    INTEGER                    -- Unix ms; NULL if open / daemon died
         );
         CREATE TABLE IF NOT EXISTS samples (
             id          INTEGER PRIMARY KEY,
             session_id  INTEGER NOT NULL REFERENCES sessions(id),
             ts          INTEGER NOT NULL,          -- Unix ms
             bpm         INTEGER NOT NULL,
             confidence  INTEGER NOT NULL           -- payload[2]; >= 128 = locked
         );
         CREATE INDEX IF NOT EXISTS samples_ts      ON samples(ts);
         CREATE INDEX IF NOT EXISTS samples_session ON samples(session_id);",
    )?;
    c.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

fn format_mac(mac: u64) -> String {
    (0..6)
        .rev()
        .map(|i| format!("{:02x}", (mac >> (i * 8)) & 0xff))
        .collect::<Vec<_>>()
        .join(":")
}

fn writer(conn: Connection, rx: mpsc::Receiver<Event>, log: fn(&str)) {
    // (session id, device) of the open session.
    let mut open: Option<(i64, u64)> = None;
    for ev in rx {
        let res: rusqlite::Result<()> = (|| {
            match ev {
                Event::Sample {
                    device,
                    ts_ms,
                    bpm,
                    confidence,
                } => {
                    // A different pair of buds starts its own session.
                    if let Some((id, dev)) = open {
                        if dev != device {
                            conn.execute(
                                "UPDATE sessions SET ended_at = ?1 WHERE id = ?2",
                                params![ts_ms, id],
                            )?;
                            open = None;
                        }
                    }
                    let session = match open {
                        Some((id, _)) => id,
                        None => {
                            conn.execute(
                                "INSERT INTO sessions (device, started_at) VALUES (?1, ?2)",
                                params![format_mac(device), ts_ms],
                            )?;
                            let id = conn.last_insert_rowid();
                            open = Some((id, device));
                            id
                        }
                    };
                    conn.execute(
                        "INSERT INTO samples (session_id, ts, bpm, confidence) VALUES (?1, ?2, ?3, ?4)",
                        params![session, ts_ms, bpm, confidence],
                    )?;
                }
                Event::EndSession { ts_ms } => {
                    if let Some((id, _)) = open.take() {
                        conn.execute(
                            "UPDATE sessions SET ended_at = ?1 WHERE id = ?2",
                            params![ts_ms, id],
                        )?;
                    }
                }
            }
            Ok(())
        })();
        if let Err(e) = res {
            log(&format!("hrdb: write failed: {e}"));
        }
    }
}
