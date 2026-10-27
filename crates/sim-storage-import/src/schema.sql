CREATE TABLE devices (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    address TEXT NOT NULL COLLATE NOCASE,
    port INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
    username TEXT NOT NULL,
    manufacturer TEXT NOT NULL CHECK (manufacturer IN ('dahua', 'jufeng')),
    password_dpapi BLOB NOT NULL,
    UNIQUE (address, port, username, manufacturer)
);
CREATE TABLE channels (
    device_id INTEGER NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    number INTEGER NOT NULL CHECK (number BETWEEN 0 AND 65535),
    name TEXT NOT NULL,
    PRIMARY KEY (device_id, number)
);
PRAGMA user_version = 1;
