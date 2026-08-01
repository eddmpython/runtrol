//! Durable paired-device authorization rows.
//!
//! # What is stored
//!
//! The locally minted device id, authenticated Noise public key, a one-way bearer-token fingerprint, validated
//! display labels, exact stable scope strings, and pairing time. The bearer token itself is never accepted by this
//! crate, so it cannot be written accidentally. Conversation content has no field and no table here.
//!
//! # Why damaged rows stop startup
//!
//! A damaged session pointer can be isolated while the other sessions remain useful. A damaged authorization row
//! is different: ignoring one silently changes who holds what. Listing still returns every readable row and every
//! error, but daemon assembly refuses to open a remote listener when `unreadable` is nonempty.

use redb::ReadableDatabase as _;
use runtrol_provider::WallMs;

use crate::error::StoreError;
use crate::open::Store;
use crate::schema::{DEVICES, DeviceKey, SCHEMA_VERSION};

const KEY_BYTES: usize = 32;
const FINGERPRINT_BYTES: usize = 32;

/// Everything runtrol stores about one paired device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRow {
    /// The authenticated X25519 public key pinned during pairing.
    pub remote_static_key: [u8; KEY_BYTES],
    /// Domain-separated SHA-256 image of the random bearer token.
    pub credential_fingerprint: [u8; FINGERPRINT_BYTES],
    /// Operator-facing device name, validated again when restored.
    pub name: Box<str>,
    /// Operator-facing platform, validated again when restored.
    pub platform: Box<str>,
    /// Exact stable `DeviceScope` renderings, parsed by daemon assembly.
    ///
    /// The store deliberately does not depend on the security crate. That missing edge prevents persistence from
    /// becoming a second authority model.
    pub scopes: Vec<Box<str>>,
    /// When exact PC presence approved this device.
    pub paired_at: WallMs,
}

impl DeviceRow {
    fn encode(&self) -> Result<Vec<u8>, StoreError> {
        let mut out = Vec::with_capacity(160);
        out.push(SCHEMA_VERSION);
        out.extend_from_slice(&self.remote_static_key);
        out.extend_from_slice(&self.credential_fingerprint);
        write_text(&mut out, "device name", &self.name)?;
        write_text(&mut out, "device platform", &self.platform)?;
        out.extend_from_slice(&self.paired_at.as_millis().to_le_bytes());

        let count = u16::try_from(self.scopes.len()).map_err(|_| StoreError::DeviceCodec {
            field: "scope count",
            why: "more than 65535 scopes cannot be represented",
        })?;
        out.extend_from_slice(&count.to_le_bytes());
        for scope in &self.scopes {
            write_text(&mut out, "scope", scope)?;
        }
        Ok(out)
    }

    fn decode(bytes: &[u8]) -> Result<Self, StoreError> {
        let mut cursor = DeviceCursor::new(bytes);
        let version = cursor.byte("row version")?;
        if version != SCHEMA_VERSION {
            return Err(StoreError::DeviceCodec {
                field: "row version",
                why: "written by a different schema version than this build understands",
            });
        }

        let remote_static_key = cursor.fixed("Noise public key")?;
        let credential_fingerprint = cursor.fixed("credential fingerprint")?;
        let name = cursor.text("device name")?.into();
        let platform = cursor.text("device platform")?.into();
        let paired_at = WallMs::from_millis(cursor.u64("paired at")?);
        let scope_count = usize::from(cursor.u16("scope count")?);
        let mut scopes = Vec::with_capacity(scope_count);
        for _ in 0..scope_count {
            scopes.push(cursor.text("scope")?.into());
        }

        if !cursor.is_finished() {
            return Err(StoreError::DeviceCodec {
                field: "end of row",
                why: "trailing bytes this build does not understand",
            });
        }

        Ok(Self {
            remote_static_key,
            credential_fingerprint,
            name,
            platform,
            scopes,
            paired_at,
        })
    }
}

impl Store {
    /// Durably save or replace one device authorization row.
    ///
    /// # Errors
    ///
    /// [`StoreError::DeviceCodec`] when a field does not fit the published row layout, or
    /// [`StoreError::Engine`] when the write fails.
    pub fn put_device(&self, device: DeviceKey, row: &DeviceRow) -> Result<(), StoreError> {
        let encoded = row.encode()?;
        let write = self.begin_durable_write("saving a device authorization")?;
        {
            let mut devices = write
                .open_table(DEVICES)
                .map_err(|error| StoreError::Engine {
                    doing: "opening the device authorization table",
                    source: Box::new(error.into()),
                })?;
            devices
                .insert(device, encoded.as_slice())
                .map_err(|error| StoreError::Engine {
                    doing: "writing a device authorization row",
                    source: Box::new(error.into()),
                })?;
        }
        write.commit().map_err(|error| StoreError::Engine {
            doing: "committing a device authorization",
            source: Box::new(error.into()),
        })
    }

    /// Read one device authorization row.
    ///
    /// # Errors
    ///
    /// [`StoreError::Engine`] when the read fails, or [`StoreError::DeviceCodec`] when the row is malformed.
    pub fn get_device(&self, device: DeviceKey) -> Result<Option<DeviceRow>, StoreError> {
        let read = self.db().begin_read().map_err(|error| StoreError::Engine {
            doing: "starting a device authorization read",
            source: Box::new(error.into()),
        })?;
        let table = match read.open_table(DEVICES) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(error) => {
                return Err(StoreError::Engine {
                    doing: "opening the device authorization table",
                    source: Box::new(error.into()),
                });
            }
        };
        let stored = table.get(device).map_err(|error| StoreError::Engine {
            doing: "reading a device authorization row",
            source: Box::new(error.into()),
        })?;
        match stored {
            None => Ok(None),
            Some(value) => DeviceRow::decode(value.value()).map(Some),
        }
    }

    /// Every stored paired device, oldest first, plus every damaged row.
    ///
    /// # Errors
    ///
    /// [`StoreError::Engine`] when the scan itself fails.
    pub fn list_devices(&self) -> Result<ListedDevices, StoreError> {
        let read = self.db().begin_read().map_err(|error| StoreError::Engine {
            doing: "starting a device authorization scan",
            source: Box::new(error.into()),
        })?;
        let table = match read.open_table(DEVICES) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(ListedDevices::default()),
            Err(error) => {
                return Err(StoreError::Engine {
                    doing: "opening the device authorization table",
                    source: Box::new(error.into()),
                });
            }
        };

        let mut listed = ListedDevices::default();
        let range = table
            .range(DeviceKey::FIRST..=DeviceKey::LAST)
            .map_err(|error| StoreError::Engine {
                doing: "scanning the device authorization table",
                source: Box::new(error.into()),
            })?;
        for entry in range {
            let (key, value) = entry.map_err(|error| StoreError::Engine {
                doing: "reading a device authorization during a scan",
                source: Box::new(error.into()),
            })?;
            let device = key.value();
            match DeviceRow::decode(value.value()) {
                Ok(row) => listed.devices.push((device, row)),
                Err(error) => listed.unreadable.push((device, error)),
            }
        }
        Ok(listed)
    }

    /// Durably revoke one paired device.
    ///
    /// Returns whether a row existed, so the caller cannot report a revocation that removed nothing.
    ///
    /// # Errors
    ///
    /// [`StoreError::Engine`] when the write fails.
    pub fn remove_device(&self, device: DeviceKey) -> Result<bool, StoreError> {
        let write = self.begin_durable_write("revoking a device authorization")?;
        let removed;
        {
            let mut devices = write
                .open_table(DEVICES)
                .map_err(|error| StoreError::Engine {
                    doing: "opening the device authorization table",
                    source: Box::new(error.into()),
                })?;
            removed = devices
                .remove(device)
                .map_err(|error| StoreError::Engine {
                    doing: "removing a device authorization row",
                    source: Box::new(error.into()),
                })?
                .is_some();
        }
        write.commit().map_err(|error| StoreError::Engine {
            doing: "committing a device revocation",
            source: Box::new(error.into()),
        })?;
        Ok(removed)
    }
}

/// Device authorization rows that decoded and rows that need operator repair.
#[derive(Debug, Default)]
pub struct ListedDevices {
    /// Readable devices, oldest first.
    pub devices: Vec<(DeviceKey, DeviceRow)>,
    /// Rows whose authority cannot be reconstructed exactly.
    pub unreadable: Vec<(DeviceKey, StoreError)>,
}

fn write_text(out: &mut Vec<u8>, field: &'static str, text: &str) -> Result<(), StoreError> {
    let len = u16::try_from(text.len()).map_err(|_| StoreError::DeviceCodec {
        field,
        why: "longer than 65535 bytes, which this field cannot describe",
    })?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(text.as_bytes());
    Ok(())
}

struct DeviceCursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> DeviceCursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    const fn is_finished(&self) -> bool {
        self.at >= self.bytes.len()
    }

    fn take(&mut self, field: &'static str, count: usize) -> Result<&'a [u8], StoreError> {
        let end = self.at.checked_add(count).ok_or(StoreError::DeviceCodec {
            field,
            why: "a length that overflows the row",
        })?;
        let slice = self
            .bytes
            .get(self.at..end)
            .ok_or(StoreError::DeviceCodec {
                field,
                why: "the row ends before this field does",
            })?;
        self.at = end;
        Ok(slice)
    }

    fn byte(&mut self, field: &'static str) -> Result<u8, StoreError> {
        self.take(field, 1)?
            .first()
            .copied()
            .ok_or(StoreError::DeviceCodec {
                field,
                why: "the row ends before this field does",
            })
    }

    fn u16(&mut self, field: &'static str) -> Result<u16, StoreError> {
        let bytes = self
            .take(field, 2)?
            .try_into()
            .map_err(|_| StoreError::DeviceCodec {
                field,
                why: "the row ends before this field does",
            })?;
        Ok(u16::from_le_bytes(bytes))
    }

    fn u64(&mut self, field: &'static str) -> Result<u64, StoreError> {
        let bytes = self
            .take(field, 8)?
            .try_into()
            .map_err(|_| StoreError::DeviceCodec {
                field,
                why: "the row ends before this field does",
            })?;
        Ok(u64::from_le_bytes(bytes))
    }

    fn fixed<const N: usize>(&mut self, field: &'static str) -> Result<[u8; N], StoreError> {
        self.take(field, N)?
            .try_into()
            .map_err(|_| StoreError::DeviceCodec {
                field,
                why: "the row ends before this field does",
            })
    }

    fn text(&mut self, field: &'static str) -> Result<&'a str, StoreError> {
        let len = usize::from(self.u16(field)?);
        core::str::from_utf8(self.take(field, len)?).map_err(|_| StoreError::DeviceCodec {
            field,
            why: "not valid UTF-8",
        })
    }
}

#[cfg(test)]
mod tests {
    use runtrol_provider::AbsPath;

    use super::*;

    struct Scratch {
        root: AbsPath,
        store: Store,
    }

    impl Scratch {
        fn make(name: &str) -> Self {
            let base = std::env::temp_dir().join(format!("runtrol-devices-{name}"));
            if base.exists() {
                std::fs::remove_dir_all(&base).expect("clear the previous run");
            }
            std::fs::create_dir_all(&base).expect("create the scratch directory");
            let root = AbsPath::canonicalize(base.to_str().expect("temp dir is UTF-8"))
                .expect("canonicalize");
            let store = Store::open(&root.join("runtrol.redb").expect("valid file name"))
                .expect("a fresh database must open");
            Self { root, store }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            if let Err(error) = std::fs::remove_dir_all(self.root.as_std_path()) {
                eprintln!("could not clean up {}: {error}", self.root);
            }
        }
    }

    fn a_row(name: &str) -> DeviceRow {
        DeviceRow {
            remote_static_key: [0x11; KEY_BYTES],
            credential_fingerprint: [0x22; FINGERPRINT_BYTES],
            name: name.into(),
            platform: "Android".into(),
            scopes: vec!["session.list".into(), "provider(example)".into()],
            paired_at: WallMs::from_millis(1_767_225_600_000),
        }
    }

    fn key(byte: u8) -> DeviceKey {
        let mut bytes = [0_u8; 16];
        bytes[0] = byte;
        DeviceKey::from_bytes(bytes)
    }

    #[test]
    fn a_device_authorization_round_trips() {
        let scratch = Scratch::make("roundtrip");
        let device = key(1);
        let row = a_row("Pixel 9");

        scratch.store.put_device(device, &row).expect("stored");
        assert_eq!(
            scratch.store.get_device(device).expect("readable"),
            Some(row)
        );
    }

    #[test]
    fn devices_list_oldest_first_without_a_read_time_sort() {
        let scratch = Scratch::make("order");
        scratch
            .store
            .put_device(key(3), &a_row("third"))
            .expect("stored");
        scratch
            .store
            .put_device(key(1), &a_row("first"))
            .expect("stored");
        scratch
            .store
            .put_device(key(2), &a_row("second"))
            .expect("stored");

        let listed = scratch.store.list_devices().expect("listable");
        let order: Vec<DeviceKey> = listed.devices.iter().map(|(id, _)| *id).collect();
        assert_eq!(order, vec![key(1), key(2), key(3)]);
        assert!(listed.unreadable.is_empty());
    }

    #[test]
    fn revocation_removes_the_complete_device_row() {
        let scratch = Scratch::make("revoke");
        let device = key(4);
        scratch
            .store
            .put_device(device, &a_row("phone"))
            .expect("stored");

        assert!(scratch.store.remove_device(device).expect("revoked"));
        assert_eq!(scratch.store.get_device(device).expect("readable"), None);
        assert!(
            !scratch
                .store
                .remove_device(device)
                .expect("idempotent lookup")
        );
    }

    #[test]
    fn one_damaged_authorization_is_reported_beside_readable_rows() {
        let scratch = Scratch::make("damaged");
        let good = key(5);
        let bad = key(6);
        scratch
            .store
            .put_device(good, &a_row("good"))
            .expect("stored");

        let write = scratch
            .store
            .begin_durable_write("planting a damaged device row")
            .expect("write");
        {
            let mut devices = write.open_table(DEVICES).expect("device table");
            devices
                .insert(bad, [0xFF_u8, 0xFF].as_slice())
                .expect("insert");
        }
        write.commit().expect("commit");

        let listed = scratch.store.list_devices().expect("listable");
        assert_eq!(listed.devices.len(), 1);
        assert_eq!(listed.unreadable.len(), 1);
        assert_eq!(listed.unreadable.first().map(|(id, _)| *id), Some(bad));
        assert!(
            listed
                .unreadable
                .first()
                .is_some_and(|(_, error)| error.needs_the_operator())
        );
    }

    #[test]
    fn trailing_bytes_and_oversized_fields_are_refused() {
        let mut encoded = a_row("phone").encode().expect("encodable");
        encoded.push(0);
        assert!(matches!(
            DeviceRow::decode(&encoded),
            Err(StoreError::DeviceCodec {
                field: "end of row",
                ..
            })
        ));

        let mut oversized = a_row("phone");
        oversized.name = "x".repeat(usize::from(u16::MAX) + 1).into();
        assert!(oversized.encode().is_err());
    }
}
