//! A planted demo corpus: one Claude Code-style transcript full of fake
//! keys, a database URL and an email, generated at run time into a private
//! temp agent-data root. Every value is assembled from a prefix plus random
//! characters here, so no source file (and no committed fixture) ever
//! contains something that looks like a credential.

use std::path::PathBuf;

/// The planted session's id and project directory name.
pub const SESSION_ID: &str = "00000000-0000-4000-8000-0000000000aa";
pub const PROJECT: &str = "-planted-demo";

/// A planted corpus on disk plus the fake values inside it.
pub struct Planted {
    pub root: PathBuf,
    /// Every planted sensitive value, so callers can prove none survives.
    pub values: Vec<String>,
    /// The transcript file BotSpy will report as provenance.
    pub transcript: PathBuf,
}

impl Planted {
    /// Remove the temp root (the example calls this when it is done).
    pub fn remove(&self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

struct Chars(u64);

impl Chars {
    fn pick(&mut self, alphabet: &str, len: usize) -> String {
        let alphabet: Vec<char> = alphabet.chars().collect();
        (0..len)
            .map(|_| {
                // xorshift64*: plenty random for a fake value.
                self.0 ^= self.0 >> 12;
                self.0 ^= self.0 << 25;
                self.0 ^= self.0 >> 27;
                let word = self.0.wrapping_mul(0x2545_f491_4f6c_dd1d);
                alphabet[(word >> 33) as usize % alphabet.len()]
            })
            .collect()
    }
}

/// AWS key ids use base32 (A-Z, 2-7), which is what scanners match.
const BASE32: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
const ALNUM: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
const LOWER: &str = "abcdefghijklmnopqrstuvwxyz";

/// A random fake AWS-style access key id: `AKIA` plus 16 characters.
pub fn fake_aws_key(seed: u64) -> String {
    format!("{}{}", "AKI", "A") + &Chars(seed | 1).pick(BASE32, 16)
}

/// A random fake GitHub-style token: `ghp_` plus 36 characters.
pub fn fake_github_token(seed: u64) -> String {
    format!("{}_{}", "ghp", Chars(seed | 1).pick(ALNUM, 36))
}

/// A random fake database URL with an embedded password.
pub fn fake_database_url(seed: u64) -> String {
    let mut chars = Chars(seed | 1);
    let password = chars.pick(ALNUM, 16);
    format!(
        "{}://app:{password}@db.acme-prod.co:5432/app",
        "postgres"
    )
}

/// A random fake personal email. Scanners deliberately ignore reserved
/// test domains such as `example.test`, so the demo uses an ordinary one.
pub fn fake_email(seed: u64) -> String {
    let mut chars = Chars(seed | 1);
    format!(
        "{}.{}@acme-mail.co",
        chars.pick(LOWER, 6),
        chars.pick(LOWER, 8)
    )
}

fn seed() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |elapsed| elapsed.as_nanos() as u64);
    nanos ^ (u64::from(std::process::id()) << 32)
}

/// A fresh private (0700 on Unix) directory under the system temp dir.
pub fn private_temp_dir(label: &str) -> std::io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("botspy-{label}-{:x}", seed()));
    std::fs::create_dir(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

/// Write the planted Claude Code transcript under a fresh temp root.
pub fn plant() -> std::io::Result<Planted> {
    let root = private_temp_dir("privacy")?;
    let base = seed();
    let aws = fake_aws_key(base);
    let github = fake_github_token(base.rotate_left(7));
    let database = fake_database_url(base.rotate_left(13));
    let email = fake_email(base.rotate_left(21));
    let project = root.join("claude_code/projects").join(PROJECT);
    std::fs::create_dir_all(&project)?;
    let transcript = project.join(format!("{SESSION_ID}.jsonl"));
    let turns = [
        (
            "user",
            format!("thanks for the help, my AWS key is {aws} please wire up the deploy"),
        ),
        ("assistant", "on it, I will read the env file".to_string()),
        (
            "user",
            format!("DATABASE_URL={database}\nGITHUB_TOKEN={github}"),
        ),
        ("assistant", "done, the pipeline is green".to_string()),
        ("user", format!("great. ping {email} when it ships")),
    ];
    let mut body = String::new();
    for (index, (role, text)) in turns.iter().enumerate() {
        let text = super::scan::json_string(text);
        let content = if *role == "user" {
            text
        } else {
            format!("[{{\"type\":\"text\",\"text\":{text}}}]")
        };
        body.push_str(&format!(
            "{{\"type\":\"{role}\",\"uuid\":\"p{index}\",\"parentUuid\":\"\",\"timestamp\":\"2026-10-01T09:00:{index:02}Z\",\"message\":{{\"role\":\"{role}\",\"content\":{content}}}}}\n"
        ));
    }
    std::fs::write(&transcript, body)?;
    Ok(Planted {
        root,
        values: vec![aws, github, database, email],
        transcript,
    })
}
