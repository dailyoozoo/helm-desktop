use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use tokio::process::Command;

/// 订阅技能镜像同步的结果计数（复制/更新/删除）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SkillSyncResult {
    pub copied: usize,
    pub updated: usize,
    pub deleted: usize,
}

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

    /// 把真实用户目录的技能镜像同步进订阅隔离目录（变更-36，解决订阅会话调不到自定义技能）。
    ///
    /// 源：Codex → `~/.codex/skills`；Claude → `~/.claude/skills`。
    /// 目标：`cli-profiles/<engine>-subscription/skills/`。
    /// 排除 `.system`（Codex 运行时自维护）与 `.helm-disabled`（Claude 停用区）；
    /// 只操作 `skills` 子树，绝不触碰 `auth.json` 及任何凭据/配置。
    pub fn sync_user_skills(&self, engine: &str) -> Result<SkillSyncResult, String> {
        let source = user_skills_dir(engine)?;
        let target = self.profile_dir(engine)?.join("skills");
        sync_skills_dir(&source, &target)
    }
}

/// 订阅隔离需要镜像的技能目录保护项：Codex 运行时自维护的 `.system`、
/// Claude 停用区 `.helm-disabled`。两者不复制、不覆盖、不删除。
const PROTECTED_SKILL_DIR_NAMES: [&str; 2] = [".system", ".helm-disabled"];

fn is_protected_skill_dir(name: &str) -> bool {
    PROTECTED_SKILL_DIR_NAMES
        .iter()
        .any(|protected| *protected == name)
}

fn user_skills_dir(engine: &str) -> Result<PathBuf, String> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .map_err(|_| "无法获取用户目录".to_string())?;
    match engine {
        "claude-code" => Ok(home.join(".claude").join("skills")),
        "codex" => Ok(home.join(".codex").join("skills")),
        other => Err(format!("未知引擎：{other}")),
    }
}

/// 镜像同步：目标缺失或源内容更新 → 复制/换新；源已删除 → 目标镜像删除。
/// 源目录不存在时静默成功（不阻断会话启动）。
fn sync_skills_dir(source: &Path, target: &Path) -> Result<SkillSyncResult, String> {
    let mut result = SkillSyncResult::default();
    if !source.is_dir() {
        return Ok(result);
    }
    fs::create_dir_all(target).map_err(|e| format!("创建订阅技能目录失败：{e}"))?;

    let mut source_names = HashSet::new();
    let source_entries = fs::read_dir(source)
        .map_err(|e| format!("读取订阅技能源目录失败：{e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取订阅技能源目录项失败：{e}"))?;
    for entry in source_entries {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.is_empty() || is_protected_skill_dir(&name) {
            continue;
        }
        let from = entry.path();
        if !from.is_dir() {
            continue;
        }
        source_names.insert(name.clone());
        let to = target.join(&name);
        if !to.exists() {
            copy_skill_dir_recursive(&from, &to)?;
            result.copied += 1;
        } else if dir_content_differs(&from, &to) {
            // 整目录换新：先复制到暂存名再替换，避免半成品目录常驻隔离目录。
            let staging = target.join(format!(".helm-sync-{name}"));
            if staging.exists() {
                fs::remove_dir_all(&staging)
                    .map_err(|e| format!("清理订阅技能暂存目录失败：{e}"))?;
            }
            copy_skill_dir_recursive(&from, &staging)?;
            fs::remove_dir_all(&to).map_err(|e| format!("替换订阅技能目录失败：{e}"))?;
            fs::rename(&staging, &to).map_err(|e| format!("落位订阅技能目录失败：{e}"))?;
            result.updated += 1;
        }
    }

    // 镜像删除：目标中存在但源已不存在的技能目录（保护项跳过）。
    let target_entries = fs::read_dir(target)
        .map_err(|e| format!("读取订阅技能目标目录失败：{e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取订阅技能目标目录项失败：{e}"))?;
    for entry in target_entries {
        let name = entry.file_name().to_string_lossy().to_string();
        if is_protected_skill_dir(&name) {
            continue;
        }
        if !source_names.contains(&name) {
            fs::remove_dir_all(entry.path())
                .map_err(|e| format!("删除订阅技能残留目录失败：{e}"))?;
            result.deleted += 1;
        }
    }
    Ok(result)
}

/// 比较两棵技能目录树：任一文件缺失、长度不同或源文件更新，即视为内容有差异。
fn dir_content_differs(source: &Path, target: &Path) -> bool {
    let Ok(source_entries) = fs::read_dir(source) else {
        return false;
    };
    let Ok(target_entries) = fs::read_dir(target) else {
        return true;
    };
    let mut target_names: HashSet<std::ffi::OsString> = target_entries
        .flatten()
        .map(|entry| entry.file_name())
        .collect();
    for entry in source_entries.flatten() {
        let from = entry.path();
        let name = entry.file_name();
        let to = target.join(&name);
        let Ok(metadata) = fs::metadata(&from) else {
            return true;
        };
        if metadata.is_dir() {
            if !to.is_dir() || dir_content_differs(&from, &to) {
                return true;
            }
        } else if metadata.is_file() {
            match (fs::metadata(&from), fs::metadata(&to)) {
                (Ok(src_meta), Ok(dst_meta)) => {
                    if src_meta.len() != dst_meta.len()
                        || src_meta.modified().ok() > dst_meta.modified().ok()
                    {
                        return true;
                    }
                }
                _ => return true,
            }
        }
        target_names.remove(&name);
    }
    // 目标多出的条目（镜像语义：源已删除/多出的文件也算差异，换新时一并清除）。
    !target_names.is_empty()
}

/// 递归复制技能目录（`fs::metadata` 跟随符号链接，按目标内容复制）。
fn copy_skill_dir_recursive(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination).map_err(|e| format!("创建订阅技能子目录失败：{e}"))?;
    for entry in fs::read_dir(source).map_err(|e| format!("读取订阅技能源目录失败：{e}"))?
    {
        let entry = entry.map_err(|e| format!("读取订阅技能源目录项失败：{e}"))?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        let metadata =
            fs::metadata(&from).map_err(|e| format!("读取订阅技能源目录项类型失败：{e}"))?;
        if metadata.is_dir() {
            copy_skill_dir_recursive(&from, &to)?;
        } else if metadata.is_file() {
            fs::copy(&from, &to).map_err(|e| format!("复制订阅技能文件失败：{e}"))?;
        }
    }
    Ok(())
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

    fn temp_skills_source(label: &str) -> PathBuf {
        temp_config_dir(label).join("user-home/.codex/skills")
    }

    fn write_skill(dir: &Path, name: &str, content: &str) -> PathBuf {
        let skill_dir = dir.join(name);
        fs::create_dir_all(&skill_dir).unwrap();
        let meta = skill_dir.join("SKILL.md");
        fs::write(&meta, content).unwrap();
        skill_dir
    }

    /// 把文件 mtime 推到未来，确保`源内容更新`判定确定成立（与目标复制时间无关）。
    fn bump_mtime_future(path: &Path) {
        let file = fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() + std::time::Duration::from_secs(60))
            .unwrap();
    }

    #[test]
    fn sync_copies_new_skills_into_isolation_directory() {
        let config_dir = temp_config_dir("sync-new");
        let store = SubscriptionProfileStore::new(config_dir.clone());
        let source = temp_skills_source("sync-new");
        let skill = write_skill(&source, "neat-freak", "# Neat Freak");
        fs::write(skill.join("helper.sh"), "echo hi").unwrap();

        let target = store.profile_dir("codex").unwrap().join("skills");
        let result = sync_skills_dir(&source, &target).unwrap();

        assert_eq!(
            result,
            SkillSyncResult {
                copied: 1,
                updated: 0,
                deleted: 0
            }
        );
        assert!(target.join("neat-freak/SKILL.md").is_file());
        assert_eq!(
            fs::read_to_string(target.join("neat-freak/SKILL.md")).unwrap(),
            "# Neat Freak"
        );
        assert!(target.join("neat-freak/helper.sh").is_file());
        let _ = fs::remove_dir_all(config_dir);
    }

    #[test]
    fn sync_overwrites_modified_skills_but_skips_unchanged() {
        let config_dir = temp_config_dir("sync-update");
        let store = SubscriptionProfileStore::new(config_dir.clone());
        let source = temp_skills_source("sync-update");
        let skill = write_skill(&source, "doc", "# Doc v1");
        let meta = skill.join("SKILL.md");
        let target = store.profile_dir("codex").unwrap().join("skills");

        assert_eq!(
            sync_skills_dir(&source, &target).unwrap(),
            SkillSyncResult {
                copied: 1,
                updated: 0,
                deleted: 0
            }
        );
        // 内容未变 → 不重复写入
        assert_eq!(
            sync_skills_dir(&source, &target).unwrap(),
            SkillSyncResult {
                copied: 0,
                updated: 0,
                deleted: 0
            }
        );
        // 修改技能 → 目标被新内容覆盖
        fs::write(&meta, "# Doc v2").unwrap();
        bump_mtime_future(&meta);
        assert_eq!(
            sync_skills_dir(&source, &target).unwrap(),
            SkillSyncResult {
                copied: 0,
                updated: 1,
                deleted: 0
            }
        );
        assert_eq!(
            fs::read_to_string(target.join("doc/SKILL.md")).unwrap(),
            "# Doc v2"
        );
        let _ = fs::remove_dir_all(config_dir);
    }

    #[test]
    fn sync_mirror_deletes_removed_skills_and_keeps_protected_dirs() {
        let config_dir = temp_config_dir("sync-delete");
        let store = SubscriptionProfileStore::new(config_dir.clone());
        let source = temp_skills_source("sync-delete");
        write_skill(&source, "a", "# A");
        write_skill(&source, "b", "# B");
        let target = store.profile_dir("codex").unwrap().join("skills");
        sync_skills_dir(&source, &target).unwrap();

        // 目标预置一个源已不存在的技能 + 保护目录（模拟 Codex 登录自生成的 .system）
        write_skill(&target, "stale", "# Stale");
        write_skill(&target, ".system", "# should stay");
        write_skill(&target, ".helm-disabled", "# should stay");
        fs::create_dir_all(target.join(".system/imagegen")).unwrap();
        fs::write(target.join(".system/imagegen/SKILL.md"), "# ImageGen").unwrap();

        let result = sync_skills_dir(&source, &target).unwrap();

        assert_eq!(
            result,
            SkillSyncResult {
                copied: 0,
                updated: 0,
                deleted: 1
            }
        );
        assert!(!target.join("stale").exists());
        assert!(target.join(".system/imagegen/SKILL.md").is_file());
        assert!(target.join(".helm-disabled/SKILL.md").is_file());
        let _ = fs::remove_dir_all(config_dir);
    }

    #[test]
    fn sync_never_copies_or_overrides_protected_dirs() {
        let config_dir = temp_config_dir("sync-protected");
        let store = SubscriptionProfileStore::new(config_dir.clone());
        let source = temp_skills_source("sync-protected");
        write_skill(&source, ".system", "# builtin");
        write_skill(&source, ".helm-disabled", "# disabled");
        let target = store.profile_dir("codex").unwrap().join("skills");
        // 目标既有保护项（内容与源不同），不得被覆盖
        write_skill(&target, ".system", "# existing");
        fs::create_dir_all(target.join(".system/imagegen")).unwrap();
        fs::write(target.join(".system/imagegen/SKILL.md"), "# ImageGen").unwrap();

        let result = sync_skills_dir(&source, &target).unwrap();

        assert_eq!(
            result,
            SkillSyncResult {
                copied: 0,
                updated: 0,
                deleted: 0
            }
        );
        assert!(!target.join("builtin").exists());
        assert!(!target.join("disabled").exists());
        assert_eq!(
            fs::read_to_string(target.join(".system/imagegen/SKILL.md")).unwrap(),
            "# ImageGen"
        );
        let _ = fs::remove_dir_all(config_dir);
    }

    #[test]
    fn sync_keeps_credentials_out_of_isolation_skills_tree() {
        let config_dir = temp_config_dir("sync-credentials");
        let store = SubscriptionProfileStore::new(config_dir.clone());
        let source = temp_skills_source("sync-credentials");
        write_skill(&source, "web-access", "# Web Access");
        fs::write(source.join("auth.json"), "credential-sentinel").unwrap();

        let target = store.profile_dir("codex").unwrap().join("skills");
        sync_skills_dir(&source, &target).unwrap();

        assert!(target.join("web-access/SKILL.md").is_file());
        assert!(!target.join("auth.json").exists());
        assert_eq!(
            fs::read_to_string(source.join("auth.json")).unwrap(),
            "credential-sentinel"
        );
        let _ = fs::remove_dir_all(config_dir);
    }

    #[test]
    fn sync_succeeds_silently_without_source_directory() {
        let config_dir = temp_config_dir("sync-no-source");
        let store = SubscriptionProfileStore::new(config_dir.clone());
        let source = config_dir.join("missing-home/.codex/skills");
        let target = store.profile_dir("codex").unwrap().join("skills");

        let result = sync_skills_dir(&source, &target).unwrap();

        assert_eq!(
            result,
            SkillSyncResult {
                copied: 0,
                updated: 0,
                deleted: 0
            }
        );
        assert!(!target.exists());
        let _ = fs::remove_dir_all(config_dir);
    }
}
