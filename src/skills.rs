use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SkillDescriptor {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub valid: bool,
    pub enabled: bool,
    pub scope: String,
}

const RESERVED: &[&str] = &[
    "browser",
    "computer-use",
    "shell",
    "mcp",
    "channels",
    "memory",
    "scheduler",
    "audio",
];

pub fn root(config_path: &Path) -> PathBuf {
    if let Some(value) = std::env::var_os("SAPIENS_SKILLS_DIR") {
        return PathBuf::from(value);
    }
    let local = PathBuf::from("skills");
    if local.is_dir() {
        return local;
    }
    config_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("skills")
}

pub fn discover(config_path: &Path, enabled: &[String]) -> Result<Vec<SkillDescriptor>> {
    let directory = root(config_path);
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for entry in
        fs::read_dir(&directory).with_context(|| format!("read skills {}", directory.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if !valid_name(&name) {
            continue;
        }
        let file = path.join("SKILL.md");
        let text = fs::read_to_string(&file).unwrap_or_default();
        let (declared_name, description, valid) = parse_skill(&text);
        entries.push(SkillDescriptor {
            name: if declared_name.is_empty() {
                name.clone()
            } else {
                declared_name
            },
            description,
            path,
            valid,
            enabled: enabled.iter().any(|item| item == &name),
            scope: "workspace".into(),
        });
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(entries)
}

pub fn validate(config_path: &Path, name: &str) -> Result<SkillDescriptor> {
    let normalized = normalize_name(name)?;
    let directory = root(config_path).join(&normalized);
    let text = fs::read_to_string(directory.join("SKILL.md"))
        .with_context(|| format!("skill not found: {normalized}"))?;
    let (declared_name, description, valid) = parse_skill(&text);
    if !valid {
        bail!("skill {normalized} has invalid frontmatter or unfinished instructions");
    }
    Ok(SkillDescriptor {
        name: if declared_name.is_empty() {
            normalized
        } else {
            declared_name
        },
        description,
        path: directory,
        valid,
        enabled: false,
        scope: "workspace".into(),
    })
}

pub fn create_candidate(config_path: &Path, name: &str, purpose: &str) -> Result<SkillDescriptor> {
    let normalized = normalize_name(name)?;
    if RESERVED.contains(&normalized.as_str()) {
        bail!("skill name is reserved: {normalized}");
    }
    let directory = root(config_path).join(&normalized);
    if directory.exists() {
        bail!("skill already exists: {normalized}");
    }
    fs::create_dir_all(directory.join("tests"))?;
    let description = if purpose.trim().is_empty() {
        format!("Reusable local workflow for {normalized}.")
    } else {
        purpose.trim().to_string()
    };
    let body = format!(
        "---\nname: {normalized}\ndescription: {description}\n---\n\n# {title}\n\nThis skill was generated as a candidate by skill-forge.\n\n## Purpose\n\n{description}\n\n## Safety\n\nReview permissions and add tests before enabling. External writes, destructive actions, secret input, and network calls require explicit approval.\n",
        title = title_case(&normalized),
    );
    fs::write(directory.join("SKILL.md"), body)?;
    fs::write(
        directory.join("manifest.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "name": normalized,
            "version": "0.1.0",
            "scope": "workspace",
            "status": "candidate",
            "generated_by": "skill-forge",
            "permissions": [],
        }))?,
    )?;
    validate(config_path, name)
}

pub fn suggestions(config_path: &Path) -> Result<Vec<serde_json::Value>> {
    let receipts = config_path.with_file_name("sapiens-agent-receipts.jsonl");
    if !receipts.exists() {
        return Ok(Vec::new());
    }
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for line in fs::read_to_string(receipts)?.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(action) = value.get("action").and_then(serde_json::Value::as_str) else {
            continue;
        };
        *counts.entry(action.to_string()).or_default() += 1;
    }
    Ok(counts
        .into_iter()
        .filter(|(_, count)| *count >= 3)
        .map(|(action, count)| {
            serde_json::json!({
                "action": action,
                "occurrences": count,
                "recommendation": "review and create a reusable skill"
            })
        })
        .collect())
}

fn parse_skill(text: &str) -> (String, String, bool) {
    let mut lines = text.lines();
    if lines.next() != Some("---") {
        return (String::new(), String::new(), false);
    }
    let mut name = String::new();
    let mut description = String::new();
    let mut closed = false;
    for line in lines.by_ref() {
        if line.trim() == "---" {
            closed = true;
            break;
        }
        if let Some(value) = line.strip_prefix("name:") {
            name = value.trim().trim_matches('"').to_string();
        }
        if let Some(value) = line.strip_prefix("description:") {
            description = value.trim().trim_matches('"').to_string();
        }
    }
    let body = lines.collect::<Vec<_>>().join("\n");
    let valid = closed && valid_name(&name) && !description.is_empty() && !body.contains("TODO");
    (name, description, valid)
}

fn normalize_name(name: &str) -> Result<String> {
    let name = name.trim().to_ascii_lowercase();
    if !valid_name(&name) {
        bail!("skill name must use lowercase letters, digits, and hyphens");
    }
    Ok(name)
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn title_case(value: &str) -> String {
    value
        .split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!(
                    "{}{}",
                    first.to_ascii_uppercase(),
                    chars.collect::<String>()
                ),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn creates_and_discovers_a_candidate_skill() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = std::env::temp_dir().join(format!("sapiens-skills-{}", std::process::id()));
        let config = root.join("config.toml");
        let skills_dir = root.join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        // SAFETY: tests serialize access to this process-wide variable.
        unsafe { std::env::set_var("SAPIENS_SKILLS_DIR", &skills_dir) };
        let descriptor =
            create_candidate(&config, "daily-report", "Create a daily report").unwrap();
        assert!(descriptor.valid);
        assert_eq!(
            discover(&config, &["daily-report".into()]).unwrap().len(),
            1
        );
        // SAFETY: tests serialize access to this process-wide variable.
        unsafe { std::env::remove_var("SAPIENS_SKILLS_DIR") };
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_unfinished_skill() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root =
            std::env::temp_dir().join(format!("sapiens-skills-invalid-{}", std::process::id()));
        let config = root.join("config.toml");
        let skills_dir = root.join("skills");
        std::fs::create_dir_all(skills_dir.join("bad")).unwrap();
        std::fs::write(
            skills_dir.join("bad/SKILL.md"),
            "---\nname: bad\ndescription: x\n---\nTODO",
        )
        .unwrap();
        // SAFETY: tests serialize access to this process-wide variable.
        unsafe { std::env::set_var("SAPIENS_SKILLS_DIR", &skills_dir) };
        assert!(validate(&config, "bad").is_err());
        // SAFETY: tests serialize access to this process-wide variable.
        unsafe { std::env::remove_var("SAPIENS_SKILLS_DIR") };
        let _ = std::fs::remove_dir_all(root);
    }
}
