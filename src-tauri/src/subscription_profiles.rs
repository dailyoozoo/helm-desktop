use std::fs;
use std::path::{Path, PathBuf};
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct SubscriptionProfileStore {
    root: PathBuf,
}

impl SubscriptionProfileStore {
    pub fn new(app_config_dir: PathBuf) -> Self {
        Self {
            root: app_config_dir.join("cli-profiles"),
        }
    }

    pub fn profile_dir(&self, engine: &str) -> Result<PathBuf, String> {
        let directory = match engine {
            "claude-code" => "claude-subscription",
            "codex" => "codex-subscription",
            other => return Err(format!("未知引擎：{other}")),
        };
        let path = self.root.join(directory);
        fs::create_dir_all(&path).map_err(|error| format!("创建订阅配置目录失败：{error}"))?;
        Ok(path)
    }

    pub fn command_env(&self, engine: &str) -> Result<(&'static str, PathBuf), String> {
        let key = match engine {
            "claude-code" => "CLAUDE_CONFIG_DIR",
            "codex" => "CODEX_HOME",
            other => return Err(format!("未知引擎：{other}")),
        };
        Ok((key, self.profile_dir(engine)?))
    }

    pub fn configure_command(
        &self,
        command: &mut Command,
        engine: &str,
    ) -> Result<PathBuf, String> {
        let (key, path) = self.command_env(engine)?;
        command.env(key, &path);
        Ok(path)
    }

    pub fn append_launch_env(
        &self,
        env: &mut Vec<(String, String)>,
        engine: &str,
    ) -> Result<PathBuf, String> {
        let (key, path) = self.command_env(engine)?;
        env.retain(|(name, _)| name != key);
        env.push((key.to_string(), path.to_string_lossy().to_string()));
        Ok(path)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_config_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "helm-subscription-profile-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn profiles_are_persistent_children_of_the_helm_config_directory() {
        let config_dir = temp_config_dir("paths");
        let store = SubscriptionProfileStore::new(config_dir.clone());

        let claude = store.profile_dir("claude-code").unwrap();
        let codex = store.profile_dir("codex").unwrap();

        assert_eq!(claude, config_dir.join("cli-profiles/claude-subscription"));
        assert_eq!(codex, config_dir.join("cli-profiles/codex-subscription"));
        assert!(claude.is_dir());
        assert!(codex.is_dir());
        assert_ne!(claude, codex);
        let _ = fs::remove_dir_all(config_dir);
    }

    #[test]
    fn launch_env_uses_engine_specific_isolation_variables() {
        let config_dir = temp_config_dir("env");
        let store = SubscriptionProfileStore::new(config_dir.clone());
        let mut env = vec![("CODEX_HOME".to_string(), "global-sentinel".to_string())];

        let codex = store.append_launch_env(&mut env, "codex").unwrap();
        assert_eq!(env.iter().filter(|(key, _)| key == "CODEX_HOME").count(), 1);
        assert!(env.contains(&(
            "CODEX_HOME".to_string(),
            codex.to_string_lossy().to_string()
        )));

        let claude = store.append_launch_env(&mut env, "claude-code").unwrap();
        assert!(env.contains(&(
            "CLAUDE_CONFIG_DIR".to_string(),
            claude.to_string_lossy().to_string()
        )));
        let _ = fs::remove_dir_all(config_dir);
    }

    #[test]
    fn command_configuration_overrides_inherited_global_profile_paths() {
        let config_dir = temp_config_dir("command-env");
        let store = SubscriptionProfileStore::new(config_dir.clone());
        let mut codex = Command::new("codex");
        codex.env("CODEX_HOME", "global-codex-sentinel");
        let codex_profile = store.configure_command(&mut codex, "codex").unwrap();
        let configured_codex_home = codex
            .as_std()
            .get_envs()
            .find(|(key, _)| *key == "CODEX_HOME")
            .and_then(|(_, value)| value)
            .map(PathBuf::from);
        assert_eq!(configured_codex_home, Some(codex_profile));

        let mut claude = Command::new("claude");
        claude.env("CLAUDE_CONFIG_DIR", "global-claude-sentinel");
        let claude_profile = store.configure_command(&mut claude, "claude-code").unwrap();
        let configured_claude_dir = claude
            .as_std()
            .get_envs()
            .find(|(key, _)| *key == "CLAUDE_CONFIG_DIR")
            .and_then(|(_, value)| value)
            .map(PathBuf::from);
        assert_eq!(configured_claude_dir, Some(claude_profile));
        let _ = fs::remove_dir_all(config_dir);
    }

    #[test]
    fn creating_profiles_does_not_copy_global_auth_or_configuration() {
        let config_dir = temp_config_dir("empty");
        let global_dir = config_dir.join("user-home/.codex");
        fs::create_dir_all(&global_dir).unwrap();
        fs::write(global_dir.join("auth.json"), "global-auth-sentinel").unwrap();
        fs::write(global_dir.join("config.toml"), "global-config-sentinel").unwrap();
        let store = SubscriptionProfileStore::new(config_dir.clone());

        let profile = store.profile_dir("codex").unwrap();

        assert!(!profile.join("auth.json").exists());
        assert!(!profile.join("config.toml").exists());
        assert_eq!(
            fs::read_to_string(global_dir.join("auth.json")).unwrap(),
            "global-auth-sentinel"
        );
        assert_eq!(
            fs::read_to_string(global_dir.join("config.toml")).unwrap(),
            "global-config-sentinel"
        );
        let _ = fs::remove_dir_all(config_dir);
    }
}
