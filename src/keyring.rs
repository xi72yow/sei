use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use std::collections::HashMap;
use zbus::Connection;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const COLLECTION_LABEL: &str = "sei-secrets";
const SS_DEST: &str = "org.freedesktop.secrets";
const SS_PATH: &str = "/org/freedesktop/secrets";
const SS_SERVICE: &str = "org.freedesktop.Secret.Service";
const SS_COLLECTION: &str = "org.freedesktop.Secret.Collection";
const SS_ITEM: &str = "org.freedesktop.Secret.Item";
const SS_PROPS: &str = "org.freedesktop.DBus.Properties";

struct Keyring {
    conn: Connection,
    session: OwnedObjectPath,
    collection: OwnedObjectPath,
}

impl Keyring {
    async fn connect() -> Result<Self> {
        let conn = Connection::session()
            .await
            .context("no D-Bus session found — is gnome-keyring-daemon running?")?;

        // OpenSession with plain (no crypto needed on local bus)
        let reply = conn
            .call_method(
                Some(SS_DEST), SS_PATH, Some(SS_SERVICE), "OpenSession",
                &("plain", Value::from("")),
            )
            .await?;
        let (_, session): (OwnedValue, OwnedObjectPath) = reply.body().deserialize()?;

        // Find sei-secrets collection
        let collection = find_or_create_collection(&conn, &session).await?;

        Ok(Keyring { conn, session, collection })
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

        // Secrets are stored base64-encoded (zbus newline truncation workaround)
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

        // base64-encode to work around zbus newline truncation bug
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

    async fn lock(&self) -> Result<()> {
        let objects = vec![&*self.collection];
        self.conn
            .call_method(Some(SS_DEST), SS_PATH, Some(SS_SERVICE), "Lock", &(objects,))
            .await?;
        Ok(())
    }

    async fn unlock(&self) -> Result<()> {
        let objects = vec![&*self.collection];
        self.conn
            .call_method(Some(SS_DEST), SS_PATH, Some(SS_SERVICE), "Unlock", &(objects,))
            .await?;
        Ok(())
    }
}

async fn find_or_create_collection(conn: &Connection, _session: &OwnedObjectPath) -> Result<OwnedObjectPath> {
    // List all collections
    let reply = conn
        .call_method(Some(SS_DEST), SS_PATH, Some(SS_PROPS), "Get", &(SS_SERVICE, "Collections"))
        .await?;
    let value: OwnedValue = reply.body().deserialize()?;
    let collections: Vec<OwnedObjectPath> = value.try_into()
        .map_err(|_| anyhow::anyhow!("failed to read Collections"))?;

    // Find by label
    for col_path in &collections {
        let reply = conn
            .call_method(Some(SS_DEST), &**col_path, Some(SS_PROPS), "Get", &(SS_COLLECTION, "Label"))
            .await?;
        let value: OwnedValue = reply.body().deserialize()?;
        let label: String = value.try_into().unwrap_or_default();
        if label == COLLECTION_LABEL {
            // Unlock
            let objects = vec![&**col_path];
            conn.call_method(Some(SS_DEST), SS_PATH, Some(SS_SERVICE), "Unlock", &(objects,)).await?;
            return Ok(col_path.clone());
        }
    }

    // Create — try standard CreateCollection first
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

    // Prompt was returned — try to perform it (will fail headless)
    if prompt_path.as_str() != "/" {
        conn.call_method(Some(SS_DEST), &*prompt_path, Some("org.freedesktop.Secret.Prompt"), "Prompt", &("",)).await?;
    }

    bail!("Keyring konnte nicht erstellt werden — GUI-Prompt erforderlich")
}

// --- Public API (unchanged signatures) ---

/// Entry representing one project+stage combo
#[derive(Debug, Clone)]
pub struct EnvEntry {
    pub path: String,
    pub stage: String,
    pub vars: Vec<(String, String)>,
}

/// Parse KEY=VALUE pairs from raw secret bytes
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

/// Serialize env vars to KEY=VALUE string
pub fn serialize_env_vars(vars: &[(String, String)]) -> String {
    vars.iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Sperrt die sei-secrets Collection (beim TUI-Quit)
pub async fn lock_collection() -> Result<()> {
    let kr = Keyring::connect().await?;
    kr.lock().await?;
    Ok(())
}

/// Load all entries from the keyring (TUI — kein Lock danach)
pub async fn load_all_entries() -> Result<Vec<EnvEntry>> {
    let kr = Keyring::connect().await?;
    kr.unlock().await?;
    let items = kr.get_items().await?;

    let mut entries = Vec::new();
    for item_path in &items {
        let attrs = kr.get_item_attrs(item_path).await?;
        let path = attrs.get("path").cloned().unwrap_or_default();
        let stage = attrs.get("stage").cloned().unwrap_or_else(|| "default".to_string());

        if attrs.get("type").is_some_and(|t| t == "sei-dotenv") {
            let secret = kr.get_secret(item_path).await?;
            let vars = parse_env_content(&secret);
            entries.push(EnvEntry { path, stage, vars });
        }
    }

    entries.sort_by(|a, b| a.path.cmp(&b.path).then(a.stage.cmp(&b.stage)));
    Ok(entries)
}

/// Load env vars for a specific path and stage (sei run — Lock danach)
pub async fn load_envs(path: &str, stage: &str) -> Result<Vec<(String, String)>> {
    let kr = Keyring::connect().await?;
    kr.unlock().await?;

    let attrs = HashMap::from([
        ("path", path),
        ("stage", stage),
        ("type", "sei-dotenv"),
    ]);

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

/// Save env vars for a specific path and stage
pub async fn save_envs(path: &str, stage: &str, vars: &[(String, String)]) -> Result<()> {
    let kr = Keyring::connect().await?;
    kr.unlock().await?;

    let content = serialize_env_vars(vars);
    let label = format!("dotenv: {path} [{stage}]");
    let attrs = HashMap::from([
        ("path", path),
        ("stage", stage),
        ("type", "sei-dotenv"),
    ]);

    kr.create_item(&label, attrs, content.as_bytes(), true).await?;
    Ok(())
}

/// Delete env entry for a specific path and stage
pub async fn delete_entry(path: &str, stage: &str) -> Result<()> {
    let kr = Keyring::connect().await?;
    kr.unlock().await?;

    let attrs = HashMap::from([
        ("path", path),
        ("stage", stage),
        ("type", "sei-dotenv"),
    ]);

    let items = kr.search_items(attrs).await?;

    if let Some(item_path) = items.first() {
        kr.delete_item(item_path).await?;
    } else {
        bail!("No entry found for {path} [{stage}]");
    }

    Ok(())
}

/// Import a .env file into the keyring
pub async fn import_env_file(
    env_file: &std::path::Path,
    path: &str,
    stage: &str,
) -> Result<()> {
    let content = std::fs::read(env_file)
        .with_context(|| format!("Failed to read {}", env_file.display()))?;
    let vars = parse_env_content(&content);

    if vars.is_empty() {
        bail!("No valid KEY=VALUE pairs found in {}", env_file.display());
    }

    save_envs(path, stage, &vars).await?;
    Ok(())
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
        let vars = vec![
            ("A".into(), "1".into()),
            ("B".into(), "2".into()),
        ];
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

    // Integration test — needs D-Bus + gnome-keyring (Containerfile test stage)
    #[tokio::test]
    async fn test_keyring_save_load_delete() {
        let path = "/tmp/sei-test-integration";
        let stage = "test";
        let vars = vec![
            ("TEST_KEY".into(), "test_value".into()),
            ("ANOTHER".into(), "123".into()),
        ];

        save_envs(path, stage, &vars).await.expect("save failed");

        let loaded = load_envs(path, stage).await.expect("load failed");
        assert_eq!(loaded, vars);

        let all = load_all_entries().await.expect("load_all failed");
        assert!(all.iter().any(|e| e.path == path && e.stage == stage));

        delete_entry(path, stage).await.expect("delete failed");

        let after = load_envs(path, stage).await.expect("load after delete failed");
        assert!(after.is_empty());
    }
}
