use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use futures_util::StreamExt;
use std::collections::{HashMap, HashSet};
use zbus::Connection;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const COLLECTION_LABEL: &str = "sei-secrets";
const SS_DEST: &str = "org.freedesktop.secrets";
const SS_PATH: &str = "/org/freedesktop/secrets";
const SS_SERVICE: &str = "org.freedesktop.Secret.Service";
const SS_COLLECTION: &str = "org.freedesktop.Secret.Collection";
const SS_ITEM: &str = "org.freedesktop.Secret.Item";
const SS_PROPS: &str = "org.freedesktop.DBus.Properties";

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn relative_time(timestamp: u64) -> String {
    if timestamp == 0 {
        return "–".to_string();
    }
    let now = unix_now();
    let diff = now.saturating_sub(timestamp);
    if diff < 60 {
        "just now".to_string()
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86400 {
        format!("{}h ago", diff / 3600)
    } else {
        format!("{}d ago", diff / 86400)
    }
}

pub struct Keyring {
    conn: Connection,
    session: OwnedObjectPath,
    collection: OwnedObjectPath,
}

impl Keyring {
    pub async fn connect() -> Result<Self> {
        let conn = Connection::session()
            .await
            .context("no D-Bus session found — is gnome-keyring-daemon running?")?;

        let reply = conn
            .call_method(
                Some(SS_DEST), SS_PATH, Some(SS_SERVICE), "OpenSession",
                &("plain", Value::from("")),
            )
            .await?;
        let (_, session): (OwnedValue, OwnedObjectPath) = reply.body().deserialize()?;

        let collection = find_or_create_collection(&conn, &session).await?;

        let kr = Keyring { conn, session, collection };
        kr.unlock().await?;
        Ok(kr)
    }

    async fn get_items(&self) -> Result<Vec<OwnedObjectPath>> {
        let reply = self.conn
            .call_method(
                Some(SS_DEST), &*self.collection, Some(SS_PROPS), "Get",
                &(SS_COLLECTION, "Items"),
            )
            .await?;
        let value: OwnedValue = reply.body().deserialize()?;
        let items: Vec<OwnedObjectPath> = value.try_into()
            .map_err(|_| anyhow::anyhow!("failed to read Items property"))?;
        Ok(items)
    }

    async fn get_item_attrs(&self, item: &OwnedObjectPath) -> Result<HashMap<String, String>> {
        let reply = self.conn
            .call_method(
                Some(SS_DEST), &**item, Some(SS_PROPS), "Get",
                &(SS_ITEM, "Attributes"),
            )
            .await?;
        let value: OwnedValue = reply.body().deserialize()?;
        let attrs: HashMap<String, String> = value.try_into()
            .map_err(|_| anyhow::anyhow!("failed to read Attributes"))?;
        Ok(attrs)
    }

    async fn get_secret(&self, item: &OwnedObjectPath) -> Result<Vec<u8>> {
        let reply = self.conn
            .call_method(
                Some(SS_DEST), &**item, Some(SS_ITEM), "GetSecret",
                &(&*self.session,),
            )
            .await?;
        let (_session, _params, value, _content_type): (OwnedObjectPath, Vec<u8>, Vec<u8>, String) =
            reply.body().deserialize()?;
        B64.decode(&value).context("failed to decode base64 secret")
    }

    async fn create_item(
        &self,
        label: &str,
        attrs: HashMap<&str, &str>,
        secret: &[u8],
        replace: bool,
    ) -> Result<OwnedObjectPath> {
        let mut props: HashMap<&str, Value> = HashMap::new();
        props.insert("org.freedesktop.Secret.Item.Label", Value::from(label));
        props.insert("org.freedesktop.Secret.Item.Attributes", Value::from(attrs));

        let encoded = B64.encode(secret);
        let secret_struct = (&*self.session, Vec::<u8>::new(), encoded.into_bytes(), "text/plain");

        let reply = self.conn
            .call_method(
                Some(SS_DEST), &*self.collection, Some(SS_COLLECTION), "CreateItem",
                &(props, secret_struct, replace),
            )
            .await
            .context("Failed to save to keyring")?;
        let (item_path, _prompt): (OwnedObjectPath, OwnedObjectPath) = reply.body().deserialize()?;
        Ok(item_path)
    }

    async fn delete_item(&self, item: &OwnedObjectPath) -> Result<()> {
        self.conn
            .call_method(Some(SS_DEST), &**item, Some(SS_ITEM), "Delete", &())
            .await?;
        Ok(())
    }

    async fn search_items(&self, attrs: HashMap<&str, &str>) -> Result<Vec<OwnedObjectPath>> {
        let reply = self.conn
            .call_method(
                Some(SS_DEST), &*self.collection, Some(SS_COLLECTION), "SearchItems",
                &(attrs,),
            )
            .await?;
        let items: Vec<OwnedObjectPath> = reply.body().deserialize()?;
        Ok(items)
    }

    pub async fn lock(&self) -> Result<()> {
        let objects = vec![&*self.collection];
        self.conn
            .call_method(Some(SS_DEST), SS_PATH, Some(SS_SERVICE), "Lock", &(objects,))
            .await?;
        Ok(())
    }

    async fn unlock(&self) -> Result<()> {
        let objects = vec![&*self.collection];
        let reply = self.conn
            .call_method(Some(SS_DEST), SS_PATH, Some(SS_SERVICE), "Unlock", &(objects,))
            .await?;
        let (_unlocked, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) =
            reply.body().deserialize()?;

        if prompt.as_str() != "/" {
            let mut stream = zbus::MessageStream::from(&self.conn);
            self.conn
                .call_method(
                    Some(SS_DEST), &*prompt,
                    Some("org.freedesktop.Secret.Prompt"), "Prompt",
                    &("",),
                )
                .await?;
            while let Some(msg) = stream.next().await {
                let msg = msg?;
                let header = msg.header();
                let is_completed = header.interface()
                    .is_some_and(|i| i.as_str() == "org.freedesktop.Secret.Prompt")
                    && header.member()
                    .is_some_and(|m| m.as_str() == "Completed");
                if is_completed {
                    let (dismissed, _result): (bool, OwnedValue) =
                        msg.body().deserialize()?;
                    if dismissed {
                        bail!("Keyring-Unlock abgebrochen");
                    }
                    break;
                }
            }
        }
        Ok(())
    }

    // --- ID management ---

    async fn get_all_ids(&self) -> Result<HashSet<String>> {
        let items = self.get_items().await?;
        let mut ids = HashSet::new();
        for item in &items {
            let attrs = self.get_item_attrs(item).await?;
            if let Some(id) = attrs.get("id") {
                ids.insert(id.clone());
            }
        }
        Ok(ids)
    }

    fn next_id(used: &HashSet<String>) -> String {
        for i in 1u16..=999 {
            let id = format!("{:03}", i);
            if !used.contains(&id) {
                return id;
            }
        }
        "999".to_string()
    }

    // --- Public methods ---

    pub async fn load_all_entries(&self) -> Result<Vec<EnvEntry>> {
        self.unlock().await?;
        let items = self.get_items().await?;

        let mut entries = Vec::new();
        for item_path in &items {
            let attrs = self.get_item_attrs(item_path).await?;
            if attrs.get("type").is_some_and(|t| t == "sei-dotenv") {
                let secret = self.get_secret(item_path).await?;
                let vars = parse_env_content(&secret);
                entries.push(EnvEntry {
                    id: attrs.get("id").cloned().unwrap_or_else(|| "–".to_string()),
                    name: attrs.get("name").cloned().unwrap_or_default(),
                    path: attrs.get("path").cloned().unwrap_or_default(),
                    stage: attrs.get("stage").cloned().unwrap_or_else(|| "default".to_string()),
                    vars,
                    created_at: attrs.get("created_at").and_then(|s| s.parse().ok()).unwrap_or(0),
                    updated_at: attrs.get("updated_at").and_then(|s| s.parse().ok()).unwrap_or(0),
                });
            }
        }

        entries.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(entries)
    }

    pub async fn save_envs(&self, path: &str, stage: &str, name: &str, vars: &[(String, String)]) -> Result<String> {
        self.unlock().await?;
        let now = unix_now().to_string();

        // Check for existing entry
        let search_attrs = HashMap::from([("path", path), ("stage", stage), ("type", "sei-dotenv")]);
        let existing = self.search_items(search_attrs).await?;

        let (id, created_at, existing_name) = if let Some(item) = existing.first() {
            let attrs = self.get_item_attrs(item).await?;
            let id = attrs.get("id").cloned().unwrap_or_default();
            let created = attrs.get("created_at").cloned().unwrap_or_else(|| now.clone());
            let ename = attrs.get("name").cloned().unwrap_or_default();
            // Delete old entry to prevent duplicates (replace only matches if ALL attrs are identical)
            self.delete_item(item).await?;
            (id, created, ename)
        } else {
            (String::new(), now.clone(), String::new())
        };

        let id = if id.is_empty() {
            let used = self.get_all_ids().await?;
            Self::next_id(&used)
        } else {
            id
        };

        // Use provided name, or keep existing name
        let final_name = if !name.is_empty() { name } else { &existing_name };

        let content = serialize_env_vars(vars);
        let label = format!("dotenv: {path} [{stage}]");
        let attrs = HashMap::from([
            ("path", path),
            ("stage", stage),
            ("type", "sei-dotenv"),
            ("id", id.as_str()),
            ("name", final_name),
            ("created_at", created_at.as_str()),
            ("updated_at", now.as_str()),
        ]);

        self.create_item(&label, attrs, content.as_bytes(), false).await?;
        Ok(id)
    }

    pub async fn delete_entry(&self, path: &str, stage: &str) -> Result<()> {
        self.unlock().await?;
        let attrs = HashMap::from([("path", path), ("stage", stage), ("type", "sei-dotenv")]);
        let items = self.search_items(attrs).await?;
        if let Some(item_path) = items.first() {
            self.delete_item(item_path).await?;
        } else {
            bail!("No entry found for {path} [{stage}]");
        }
        Ok(())
    }

    pub async fn import_env_file(&self, env_file: &std::path::Path, path: &str, stage: &str) -> Result<String> {
        let content = std::fs::read(env_file)
            .with_context(|| format!("Failed to read {}", env_file.display()))?;
        let vars = parse_env_content(&content);
        if vars.is_empty() {
            bail!("No valid KEY=VALUE pairs found in {}", env_file.display());
        }
        self.save_envs(path, stage, "", &vars).await
    }
}

async fn find_or_create_collection(conn: &Connection, _session: &OwnedObjectPath) -> Result<OwnedObjectPath> {
    let reply = conn
        .call_method(Some(SS_DEST), SS_PATH, Some(SS_PROPS), "Get", &(SS_SERVICE, "Collections"))
        .await?;
    let value: OwnedValue = reply.body().deserialize()?;
    let collections: Vec<OwnedObjectPath> = value.try_into()
        .map_err(|_| anyhow::anyhow!("failed to read Collections"))?;

    for col_path in &collections {
        let reply = conn
            .call_method(Some(SS_DEST), &**col_path, Some(SS_PROPS), "Get", &(SS_COLLECTION, "Label"))
            .await?;
        let value: OwnedValue = reply.body().deserialize()?;
        let label: String = value.try_into().unwrap_or_default();
        if label == COLLECTION_LABEL {
            return Ok(col_path.clone());
        }
    }

    let mut props: HashMap<&str, Value> = HashMap::new();
    props.insert("org.freedesktop.Secret.Collection.Label", Value::from(COLLECTION_LABEL));

    let reply = conn
        .call_method(Some(SS_DEST), SS_PATH, Some(SS_SERVICE), "CreateCollection", &(props, ""))
        .await
        .context("Keyring konnte nicht erstellt werden")?;
    let (col_path, prompt_path): (OwnedObjectPath, OwnedObjectPath) = reply.body().deserialize()?;

    if col_path.as_str() != "/" {
        return Ok(col_path);
    }

    if prompt_path.as_str() != "/" {
        conn.call_method(Some(SS_DEST), &*prompt_path, Some("org.freedesktop.Secret.Prompt"), "Prompt", &("",)).await?;
    }

    bail!("Keyring konnte nicht erstellt werden — GUI-Prompt erforderlich")
}

// --- Standalone helpers ---

#[derive(Debug, Clone)]
pub struct EnvEntry {
    pub id: String,
    pub name: String,
    pub path: String,
    pub stage: String,
    pub vars: Vec<(String, String)>,
    pub created_at: u64,
    pub updated_at: u64,
}

impl EnvEntry {
    /// Display name: custom name if set, otherwise last folder component
    pub fn display_name(&self) -> &str {
        if !self.name.is_empty() {
            &self.name
        } else {
            std::path::Path::new(&self.path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&self.path)
        }
    }
}

pub fn parse_env_content(content: &[u8]) -> Vec<(String, String)> {
    String::from_utf8_lossy(content)
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#')
        })
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            Some((key.trim().to_string(), value.trim().to_string()))
        })
        .collect()
}

pub fn serialize_env_vars(vars: &[(String, String)]) -> String {
    vars.iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Load env vars by ID (sei run shorthand — own connection, lock after)
pub async fn load_envs_by_id(id: &str) -> Result<Vec<(String, String)>> {
    let kr = Keyring::connect().await?;
    let attrs = HashMap::from([("id", id), ("type", "sei-dotenv")]);
    let items = kr.search_items(attrs).await?;
    let result = if let Some(item_path) = items.first() {
        let secret = kr.get_secret(item_path).await?;
        parse_env_content(&secret)
    } else {
        Vec::new()
    };
    kr.lock().await?;
    Ok(result)
}

/// Load env vars for a specific path and stage (sei run — own connection, lock after)
pub async fn load_envs(path: &str, stage: &str) -> Result<Vec<(String, String)>> {
    let kr = Keyring::connect().await?;
    let attrs = HashMap::from([("path", path), ("stage", stage), ("type", "sei-dotenv")]);
    let items = kr.search_items(attrs).await?;
    let result = if let Some(item_path) = items.first() {
        let secret = kr.get_secret(item_path).await?;
        parse_env_content(&secret)
    } else {
        Vec::new()
    };
    kr.lock().await?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_env_content() {
        let content = b"DB_HOST=localhost\nDB_PORT=5432\n";
        let vars = parse_env_content(content);
        assert_eq!(vars, vec![
            ("DB_HOST".into(), "localhost".into()),
            ("DB_PORT".into(), "5432".into()),
        ]);
    }

    #[test]
    fn test_parse_env_content_skips_comments_and_empty() {
        let content = b"# comment\n\nKEY=value\n  \n# another\nFOO=bar";
        let vars = parse_env_content(content);
        assert_eq!(vars, vec![
            ("KEY".into(), "value".into()),
            ("FOO".into(), "bar".into()),
        ]);
    }

    #[test]
    fn test_parse_env_content_with_equals_in_value() {
        let content = b"URL=postgres://user:pass@host/db?opt=1";
        let vars = parse_env_content(content);
        assert_eq!(vars, vec![
            ("URL".into(), "postgres://user:pass@host/db?opt=1".into()),
        ]);
    }

    #[test]
    fn test_serialize_env_vars() {
        let vars = vec![("A".into(), "1".into()), ("B".into(), "2".into())];
        assert_eq!(serialize_env_vars(&vars), "A=1\nB=2");
    }

    #[test]
    fn test_roundtrip_parse_serialize() {
        let original = vec![
            ("DB_HOST".into(), "localhost".into()),
            ("DB_PASS".into(), "s3cr3t".into()),
        ];
        let serialized = serialize_env_vars(&original);
        let parsed = parse_env_content(serialized.as_bytes());
        assert_eq!(original, parsed);
    }

    #[test]
    fn test_relative_time() {
        assert_eq!(relative_time(0), "–");
        let now = unix_now();
        assert_eq!(relative_time(now), "just now");
        assert_eq!(relative_time(now - 120), "2m ago");
        assert_eq!(relative_time(now - 7200), "2h ago");
        assert_eq!(relative_time(now - 172800), "2d ago");
    }

    #[tokio::test]
    async fn test_keyring_save_load_delete() {
        let kr = Keyring::connect().await.expect("connect failed");
        let path = "/tmp/sei-test-integration";
        let stage = "test";
        let vars = vec![
            ("TEST_KEY".into(), "test_value".into()),
            ("ANOTHER".into(), "123".into()),
        ];

        let id = kr.save_envs(path, stage, "test-entry", &vars).await.expect("save failed");
        assert_eq!(id.len(), 3);

        let all = kr.load_all_entries().await.expect("load_all failed");
        let entry = all.iter().find(|e| e.path == path && e.stage == stage).unwrap();
        assert_eq!(entry.vars, vars);
        assert_eq!(entry.id, id);
        assert!(entry.created_at > 0);
        assert!(entry.updated_at > 0);

        kr.delete_entry(path, stage).await.expect("delete failed");

        let after = kr.load_all_entries().await.expect("load_all after delete failed");
        assert!(!after.iter().any(|e| e.path == path && e.stage == stage));
    }

    #[tokio::test]
    async fn test_load_envs_standalone() {
        let kr = Keyring::connect().await.expect("connect failed");
        let path = "/tmp/sei-test-load-envs";
        let stage = "test";
        let vars = vec![("KEY".into(), "val".into())];

        kr.save_envs(path, stage, "", &vars).await.expect("save failed");
        drop(kr);

        let loaded = load_envs(path, stage).await.expect("load failed");
        assert_eq!(loaded, vars);

        let kr = Keyring::connect().await.expect("reconnect failed");
        kr.delete_entry(path, stage).await.expect("delete failed");
    }
}
