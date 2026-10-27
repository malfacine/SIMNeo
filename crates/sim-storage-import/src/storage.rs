use crate::{Error, NewDevice, credentials};
use rusqlite::{Connection, params};
use sim_core::{Channel, ConnectionMode, Device, DeviceDraft, Manufacturer};
use std::{path::Path, time::Duration};
use zeroize::Zeroizing;

pub struct Store {
    connection: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, Error> {
        let mut connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(3))?;
        connection.pragma_update(None, "foreign_keys", true)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        let tx = connection.transaction()?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        match version {
            0 => tx.execute_batch(include_str!("schema.sql"))?,
            1..=4 => {}
            _ => return Err(Error::NewerDatabase),
        }
        if version < 2 {
            tx.execute_batch("ALTER TABLE devices ADD COLUMN connection_mode TEXT NOT NULL DEFAULT 'address' CHECK(connection_mode IN ('address','p2p')); PRAGMA user_version=2;")?;
        }
        if version < 3 {
            tx.execute_batch("CREATE TABLE app_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL); PRAGMA user_version=3;")?;
        }
        if version < 4 {
            tx.execute_batch("ALTER TABLE devices ADD COLUMN serial TEXT; PRAGMA user_version=4;")?;
        }
        tx.commit()?;
        Ok(Self { connection })
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>, Error> {
        use rusqlite::OptionalExtension;
        Ok(self
            .connection
            .query_row("SELECT value FROM app_settings WHERE key=?1", [key], |r| {
                r.get(0)
            })
            .optional()?)
    }
    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), Error> {
        self.connection.execute("INSERT INTO app_settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,value])?;
        Ok(())
    }

    /// O lote inteiro é atômico, incluindo canais e credenciais.
    pub fn insert_batch(&mut self, devices: &[NewDevice]) -> Result<usize, Error> {
        for device in devices {
            device.config.validate()?;
        }
        let tx = self.connection.transaction()?;
        for device in devices {
            let d = &device.config;
            let protected = credentials::protect(&device.password)?;
            tx.execute(
                "INSERT INTO devices (name,address,port,username,manufacturer,password_dpapi,connection_mode,serial)
                VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    d.name.trim(),
                    d.address,
                    d.port,
                    d.username.trim(),
                    d.manufacturer.as_str(),
                    protected,
                    match d.connection {ConnectionMode::Address=>"address",ConnectionMode::P2p=>"p2p"},
                    d.serial,
                ],
            )?;
            let id = tx.last_insert_rowid();
            for channel in &d.channels {
                tx.execute(
                    "INSERT INTO channels (device_id,number,name) VALUES (?1,?2,?3)",
                    params![id, channel.number, channel.name],
                )?;
            }
        }
        tx.commit()?;
        Ok(devices.len())
    }

    pub fn list(&self) -> Result<Vec<Device>, Error> {
        let mut statement = self.connection.prepare(
            "SELECT id,name,address,port,username,manufacturer,connection_mode,serial FROM devices ORDER BY name COLLATE NOCASE,id")?;
        let devices = statement
            .query_map([], |row| {
                let manufacturer = match row.get::<_, String>(5)?.as_str() {
                    "dahua" => Manufacturer::Dahua,
                    "jufeng" => Manufacturer::Jufeng,
                    _ => return Err(rusqlite::Error::InvalidQuery),
                };
                Ok(Device {
                    id: row.get(0)?,
                    config: DeviceDraft {
                        serial: row.get(7)?,
                        connection: match row.get::<_, String>(6)?.as_str() {
                            "address" => ConnectionMode::Address,
                            "p2p" => ConnectionMode::P2p,
                            _ => return Err(rusqlite::Error::InvalidQuery),
                        },
                        name: row.get(1)?,
                        address: row.get(2)?,
                        port: row.get(3)?,
                        username: row.get(4)?,
                        manufacturer,
                        channels: vec![],
                    },
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut channel_query = self
            .connection
            .prepare("SELECT number,name FROM channels WHERE device_id=?1 ORDER BY number")?;
        let mut result = devices;
        for device in &mut result {
            device.config.normalize_serials();
            device.config.channels = channel_query
                .query_map([device.id], |r| {
                    Ok(Channel {
                        number: r.get(0)?,
                        name: r.get(1)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
        }
        Ok(result)
    }

    /// Senha vazia na edição preserva a credencial protegida existente.
    pub fn update(&mut self, id: i64, device: &NewDevice) -> Result<usize, Error> {
        device.config.validate()?;
        let tx = self.connection.transaction()?;
        let d = &device.config;
        let changed = tx.execute("UPDATE devices SET name=?1,address=?2,port=?3,username=?4,manufacturer=?5,connection_mode=?6,serial=?8 WHERE id=?7",
            params![d.name.trim(),d.address,d.port,d.username.trim(),d.manufacturer.as_str(),match d.connection { ConnectionMode::Address=>"address",ConnectionMode::P2p=>"p2p" },id,d.serial])?;
        if changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows.into());
        }
        if !device.password.is_empty() {
            let protected = credentials::protect(&device.password)?;
            tx.execute(
                "UPDATE devices SET password_dpapi=?1 WHERE id=?2",
                params![protected, id],
            )?;
        }
        tx.execute("DELETE FROM channels WHERE device_id=?1", [id])?;
        for c in &d.channels {
            tx.execute(
                "INSERT INTO channels(device_id,number,name) VALUES(?1,?2,?3)",
                params![id, c.number, c.name],
            )?;
        }
        tx.commit()?;
        Ok(changed)
    }

    pub fn delete(&mut self, id: i64) -> Result<usize, Error> {
        Ok(self
            .connection
            .execute("DELETE FROM devices WHERE id=?1", [id])?)
    }

    pub fn password(&self, id: i64) -> Result<Zeroizing<String>, Error> {
        let protected: Vec<u8> = self.connection.query_row(
            "SELECT password_dpapi FROM devices WHERE id=?1",
            [id],
            |r| r.get(0),
        )?;
        credentials::unprotect(&protected)
    }

    /// Acrescenta canais novos e preserva os nomes locais e canais existentes.
    pub fn sync_channels(&mut self, id: i64, channels: &[Channel]) -> Result<(), Error> {
        let devices = self.list()?;
        let device = devices
            .iter()
            .find(|d| d.id == id)
            .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        let mut validated = device.config.clone();
        validated.channels = channels.to_vec();
        validated.validate()?;
        let tx = self.connection.transaction()?;
        for channel in channels {
            tx.execute("INSERT INTO channels(device_id,number,name) VALUES(?1,?2,?3) ON CONFLICT(device_id,number) DO NOTHING",
                params![id,channel.number,channel.name])?;
        }
        tx.commit()?;
        Ok(())
    }
}
