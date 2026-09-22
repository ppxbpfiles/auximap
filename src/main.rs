//! auximap - PPx aux: path / Emacs IMAP bridge
//!
//! DISCLAIMER:
//! 本ツールは Paper Plane xUI (PPx) の aux: パス機能を利用した非公式の連携ツールです。
//! PPx の作者である TORO 氏の著作物ではありません。
//! 本ツールに関するお問い合わせ等を TORO 氏へ行うことはご遠慮ください。

use std::env;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use anyhow::{anyhow, Context, Result};
use chrono::Local;
use ini::Ini;
use native_tls::TlsConnector;
use mailparse::MailHeaderMap;


/// ドメインルール設定（[RULE:<pattern>]）
#[derive(Debug, Clone)]
struct DomainRule {
    pattern: String,
    host: Option<String>,
    port: Option<u16>,
    ssl: Option<bool>,
    trash_folder: Option<String>,
}

fn match_rule_pattern(pattern: &str, domain: &str) -> bool {
    let pat = pattern.to_lowercase();
    let dom = domain.to_lowercase();
    if pat == "default" || pat == "*" {
        return true;
    }
    if pat == dom {
        return true;
    }
    if let Some(suffix) = pat.strip_prefix("*.") {
        return dom.ends_with(&format!(".{}", suffix)) || dom == suffix;
    }
    if let Some(prefix) = pat.strip_suffix(".*") {
        return dom.starts_with(&format!("{}.", prefix));
    }
    false
}

fn expand_template(tmpl: &str, user: &str, domain: &str) -> String {
    let user_part = if let Some(idx) = user.find('@') {
        &user[..idx]
    } else {
        user
    };
    tmpl.replace("{domain}", domain)
        .replace("{user_part}", user_part)
        .replace("{user}", user)
}

fn get_builtin_rules() -> Vec<DomainRule> {
    vec![
        DomainRule {
            pattern: "gmail.com".to_string(),
            host: Some("imap.gmail.com".to_string()),
            port: Some(993),
            ssl: Some(true),
            trash_folder: Some("[Gmail]/ゴミ箱".to_string()),
        },
        DomainRule {
            pattern: "googlemail.com".to_string(),
            host: Some("imap.gmail.com".to_string()),
            port: Some(993),
            ssl: Some(true),
            trash_folder: Some("[Gmail]/ゴミ箱".to_string()),
        },
        DomainRule {
            pattern: "yahoo.co.jp".to_string(),
            host: Some("imap.mail.yahoo.co.jp".to_string()),
            port: Some(993),
            ssl: Some(true),
            trash_folder: Some("Trash".to_string()),
        },
        DomainRule {
            pattern: "ymail.ne.jp".to_string(),
            host: Some("imap.mail.yahoo.co.jp".to_string()),
            port: Some(993),
            ssl: Some(true),
            trash_folder: Some("Trash".to_string()),
        },
        DomainRule {
            pattern: "*.sakura.ne.jp".to_string(),
            host: Some("{domain}".to_string()),
            port: Some(993),
            ssl: Some(true),
            trash_folder: Some("Trash".to_string()),
        },
        DomainRule {
            pattern: "outlook.com".to_string(),
            host: Some("outlook.office365.com".to_string()),
            port: Some(993),
            ssl: Some(true),
            trash_folder: Some("Deleted".to_string()),
        },
        DomainRule {
            pattern: "hotmail.com".to_string(),
            host: Some("outlook.office365.com".to_string()),
            port: Some(993),
            ssl: Some(true),
            trash_folder: Some("Deleted".to_string()),
        },
        DomainRule {
            pattern: "live.com".to_string(),
            host: Some("outlook.office365.com".to_string()),
            port: Some(993),
            ssl: Some(true),
            trash_folder: Some("Deleted".to_string()),
        },
        DomainRule {
            pattern: "icloud.com".to_string(),
            host: Some("imap.mail.me.com".to_string()),
            port: Some(993),
            ssl: Some(true),
            trash_folder: Some("Deleted Messages".to_string()),
        },
        DomainRule {
            pattern: "me.com".to_string(),
            host: Some("imap.mail.me.com".to_string()),
            port: Some(993),
            ssl: Some(true),
            trash_folder: Some("Deleted Messages".to_string()),
        },
    ]
}

/// 設定ファイル内のアカウント設定
#[derive(Debug, Clone)]
struct AccountConfig {
    name: String,
    host: String,
    port: u16,
    user: String,
    pass: String,
    ssl: bool,
    trash_folder: String,
    limit: u32,
    offset: u32,
    chunk_size: u32,
    timezone: String,
    date_source: String,
}

/// メール閲覧（read）設定
#[derive(Debug, Clone)]
struct ViewConfig {
    template_file: Option<String>,
    template_inline: Option<String>,
    html_policy: String,
}

/// [VIEW] セクション設定の読み込み
fn load_view_config(ini_path: &Path) -> ViewConfig {
    if let Ok(conf) = Ini::load_from_file(ini_path) {
        let template_file = conf.get_from(Some("VIEW"), "template_file")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let template_inline = conf.get_from(Some("VIEW"), "template")
            .map(|s| s.replace("\\n", "\n").replace("\\r", "\r").to_string())
            .filter(|s| !s.is_empty());
        let html_policy = conf.get_from(Some("VIEW"), "html_policy")
            .map(|s| s.trim().to_lowercase())
            .unwrap_or_else(|| "text".to_string());

        ViewConfig {
            template_file,
            template_inline,
            html_policy,
        }
    } else {
        ViewConfig {
            template_file: None,
            template_inline: None,
            html_policy: "text".to_string(),
        }
    }
}

/// 設定ファイル（auximap.ini または imapmail.ini）の読み込み（生設定）
fn load_config_raw() -> Result<(Vec<AccountConfig>, PathBuf, Vec<DomainRule>, String)> {
    let exe_path = env::current_exe().context("Failed to get exe path")?;
    let exe_dir = exe_path.parent().unwrap_or_else(|| Path::new("."));

    let mut candidates = vec![
        exe_dir.join("auximap.ini"),
        PathBuf::from("auximap.ini"),
        exe_dir.join("imapmail.ini"),
        PathBuf::from("imapmail.ini"),
    ];

    if let Ok(appdata) = env::var("APPDATA") {
        candidates.push(PathBuf::from(&appdata).join("ppx").join("auximap.ini"));
        candidates.push(PathBuf::from(&appdata).join("ppx").join("imapmail.ini"));
    }

    let ini_path = candidates.into_iter().find(|p| p.exists())
        .ok_or_else(|| anyhow!("Config file 'auximap.ini' not found. Please create one next to auximap.exe."))?;

    let conf = Ini::load_from_file(&ini_path)
        .map_err(|e| anyhow!("Failed to parse {}: {}", ini_path.display(), e))?;

    if let Some(debug_val) = conf.get_from(Some("DEFAULT"), "debug") {
        if debug_val.eq_ignore_ascii_case("true") || debug_val == "1" {
            set_debug_mode(true);
        }
    }

    let default_limit = conf.get_from(Some("DEFAULT"), "limit")
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(100);

    let default_offset = conf.get_from(Some("DEFAULT"), "offset")
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);

    let default_trash = conf.get_from(Some("DEFAULT"), "trash_folder")
        .unwrap_or("Trash")
        .to_string();

    let default_chunk_size = conf.get_from(Some("DEFAULT"), "chunk_size")
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);

    let default_timezone = conf.get_from(Some("DEFAULT"), "timezone")
        .map(|s| s.trim().to_lowercase())
        .unwrap_or_else(|| "local".to_string());

    let default_date_source = conf.get_from(Some("DEFAULT"), "date_source")
        .map(|s| s.trim().to_lowercase())
        .unwrap_or_else(|| "internal".to_string());

    // 1. [RULE:<pattern>] セクションの収集
    let mut custom_rules = Vec::new();
    for (section, prop) in conf.iter() {
        if let Some(sec_name) = section {
            let lower = sec_name.to_lowercase();
            if let Some(rule_pat) = lower.strip_prefix("rule:") {
                let r_host = prop.get("host").map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                let r_port = prop.get("port").and_then(|s| s.trim().parse::<u16>().ok());
                let r_ssl = prop.get("ssl").map(|s| s.trim() != "false" && s.trim() != "0");
                let r_trash = prop.get("trash_folder").map(|s| s.trim().to_string()).filter(|s| !s.is_empty());

                custom_rules.push(DomainRule {
                    pattern: rule_pat.trim().to_string(),
                    host: r_host,
                    port: r_port,
                    ssl: r_ssl,
                    trash_folder: r_trash,
                });
            }
        }
    }

    // iniのルールを優先し、後ろに組み込みルールを追加
    let mut all_rules = custom_rules;
    all_rules.extend(get_builtin_rules());

    let mut accounts = Vec::new();

    for (section, prop) in conf.iter() {
        if let Some(sec_name) = section {
            let lower = sec_name.to_lowercase();
            if lower == "default" || lower == "view" || lower.starts_with("rule:") {
                continue;
            }

            let host = prop.get("host").unwrap_or("").trim().to_string();
            let port = prop.get("port").and_then(|s| s.trim().parse::<u16>().ok());
            let user = prop.get("user").unwrap_or("").trim().to_string();
            let pass = prop.get("pass").unwrap_or("").trim().to_string();
            let ssl = prop.get("ssl").map(|s| s.trim() != "false" && s.trim() != "0");
            let trash_folder = prop.get("trash_folder").map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            let limit = prop.get("limit").and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(default_limit);
            let offset = prop.get("offset").and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(default_offset);
            let chunk_size = prop.get("chunk_size").and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(default_chunk_size);
            let timezone = prop.get("timezone").map(|s| s.trim().to_lowercase()).unwrap_or_else(|| default_timezone.clone());
            let date_source = prop.get("date_source").map(|s| s.trim().to_lowercase()).unwrap_or_else(|| default_date_source.clone());

            let mut acc = AccountConfig {
                name: sec_name.to_string(),
                host,
                port: port.unwrap_or(993),
                user,
                pass,
                ssl: ssl.unwrap_or(true),
                trash_folder: trash_folder.unwrap_or_else(|| default_trash.clone()),
                limit,
                offset,
                chunk_size,
                timezone,
                date_source,
            };

            // ini に user が指定されている場合はドメインルールを事前適用
            apply_domain_rules_to_account(&mut acc, &all_rules, &default_trash);

            // host, user, pass のいずれかが指定されていれば有効なセクションとみなす
            // （user が空欄で pass のみの場合でも、CLI --user から動的に補完されるため保持）
            if !acc.host.is_empty() || !acc.user.is_empty() || !acc.pass.is_empty() {
                accounts.push(acc);
            }
        }
    }

    if accounts.is_empty() {
        return Err(anyhow!("No valid account sections found in {}", ini_path.display()));
    }

    Ok((accounts, ini_path, all_rules, default_trash))
}

/// ドメインルールをアカウント設定に適用
fn apply_domain_rules_to_account(
    acc: &mut AccountConfig,
    rules: &[DomainRule],
    default_trash: &str,
) {
    if acc.user.is_empty() {
        return;
    }
    let domain = if let Some(idx) = acc.user.find('@') {
        &acc.user[idx + 1..]
    } else {
        ""
    };
    if domain.is_empty() {
        return;
    }

    let matched_rule = rules.iter().find(|r| r.pattern.eq_ignore_ascii_case(domain))
        .or_else(|| rules.iter().find(|r| !r.pattern.eq_ignore_ascii_case("default") && !r.pattern.eq_ignore_ascii_case("*") && match_rule_pattern(&r.pattern, domain)))
        .or_else(|| rules.iter().find(|r| r.pattern.eq_ignore_ascii_case("default") || r.pattern == "*"));

    if let Some(rule) = matched_rule {
        if acc.host.is_empty() {
            if let Some(rh) = &rule.host {
                acc.host = expand_template(rh, &acc.user, domain);
            }
        }
        if acc.port == 993 || acc.port == 0 {
            if let Some(rp) = rule.port {
                acc.port = rp;
            }
        }
        if let Some(rs) = rule.ssl {
            acc.ssl = rs;
        }
        if acc.trash_folder.is_empty() || acc.trash_folder == default_trash || acc.trash_folder == "Trash" {
            if let Some(rt) = &rule.trash_folder {
                acc.trash_folder = rt.clone();
            }
        }
    }
}

/// アカウントのユーザー名（メールアドレス）解決と動的ドメイン補完
/// 1. ini に user = ... が記述されていれば最優先（変更しない）
/// 2. ini の user が空欄の場合、CLI 引数 --user（PPx %*user）を注入
/// 3. 注入後、ドメインルールにより host, port, ssl, trash_folder を自動補完
fn resolve_account_user(
    accounts: &mut [AccountConfig],
    cli_user: Option<&str>,
    target_account: Option<&str>,
    rules: &[DomainRule],
    default_trash: &str,
) -> Result<()> {
    if let Some(u) = cli_user {
        let u_trimmed = u.trim();
        if !u_trimmed.is_empty() {
            if let Some(target) = target_account {
                if let Some(acc) = accounts.iter_mut().find(|a| a.name.eq_ignore_ascii_case(target)) {
                    if acc.user.trim().is_empty() {
                        acc.user = u_trimmed.to_string();
                        apply_domain_rules_to_account(acc, rules, default_trash);
                    }
                }
            } else {
                for acc in accounts.iter_mut() {
                    if acc.user.trim().is_empty() {
                        acc.user = u_trimmed.to_string();
                        apply_domain_rules_to_account(acc, rules, default_trash);
                    }
                }
            }
        }
    }

    // 対象アカウントのユーザー名・ホスト名チェック
    if let Some(target) = target_account {
        if let Some(acc) = accounts.iter().find(|a| a.name.eq_ignore_ascii_case(target)) {
            if acc.user.trim().is_empty() {
                return Err(anyhow!(
                    "User (email address) is empty for account '{}'.\nPlease provide it via PPx (--user=\"%*user\") or configure 'user = your_email@example.com' in auximap.ini.",
                    acc.name
                ));
            }
            if acc.host.trim().is_empty() {
                return Err(anyhow!(
                    "Host is empty for account '{}' and could not be determined from email domain.\nPlease specify 'host = ...' in auximap.ini.",
                    acc.name
                ));
            }
        }
    }

    Ok(())
}

/// アカウントのパスワード解決
/// 1. CLI 引数 --pass（PPx %*pass）が最優先
/// 2. auximap.ini の pass = ...（平文）
/// 3. 環境変数 AUXIMAP_PASS_<ACCOUNT> または AUXIMAP_PASS
fn resolve_account_passwords(
    accounts: &mut [AccountConfig],
    cli_pass: Option<&str>,
    target_account: Option<&str>,
) -> Result<()> {
    // 1. CLI引数 --pass が指定されている場合、ini が空のアカウントにのみ適用
    // （ini に個別パスワードが記述されているアカウントを PPx %*pass の使い回しで破壊しない）
    if let Some(p) = cli_pass {
        let p_trimmed = p.trim();
        if !p_trimmed.is_empty() {
            if let Some(target) = target_account {
                if let Some(acc) = accounts.iter_mut().find(|a| a.name.eq_ignore_ascii_case(target)) {
                    if acc.pass.trim().is_empty() {
                        acc.pass = p_trimmed.to_string();
                    }
                }
            } else {
                for acc in accounts.iter_mut() {
                    if acc.pass.trim().is_empty() {
                        acc.pass = p_trimmed.to_string();
                    }
                }
            }
        }
    }

    // 2. ini に直接書かれているかチェック / 3. 環境変数チェック
    for acc in accounts.iter_mut() {
        if acc.pass.trim().is_empty() {
            let env_name = format!("AUXIMAP_PASS_{}", acc.name.to_uppercase());
            if let Ok(env_p) = env::var(&env_name) {
                let trimmed = env_p.trim();
                if !trimmed.is_empty() {
                    acc.pass = trimmed.to_string();
                }
            } else if let Ok(env_p) = env::var("AUXIMAP_PASS") {
                let trimmed = env_p.trim();
                if !trimmed.is_empty() {
                    acc.pass = trimmed.to_string();
                }
            }
        }
    }

    // 対象アカウントのパスワードが空であればエラー
    if let Some(target) = target_account {
        if let Some(acc) = accounts.iter().find(|a| a.name.eq_ignore_ascii_case(target)) {
            if acc.pass.trim().is_empty() {
                return Err(anyhow!(
                    "Password is empty for account '{}'.\nPlease provide it via PPx (%*pass) or configure 'pass = your_password' in auximap.ini.",
                    acc.name
                ));
            }
        }
    }

    Ok(())
}

/// 設定ファイル（auximap.ini または imapmail.ini）の読み込み
fn load_config(
    cli_pass: Option<&str>,
    cli_user: Option<&str>,
    target_account: Option<&str>,
) -> Result<(Vec<AccountConfig>, PathBuf)> {
    let (mut accounts, ini_path, all_rules, default_trash) = load_config_raw()?;
    resolve_account_user(&mut accounts, cli_user, target_account, &all_rules, &default_trash)?;
    resolve_account_passwords(&mut accounts, cli_pass, target_account)?;
    Ok((accounts, ini_path))
}

/// IMAP セッションの確立（SSL/TLS）
fn connect_imap(acc: &AccountConfig) -> Result<imap::Session<native_tls::TlsStream<std::net::TcpStream>>> {
    if !acc.ssl {
        return Err(anyhow!("Plain text (non-SSL) connection is not supported for security reasons."));
    }

    let tls = TlsConnector::builder().build().context("Failed to build TLS connector")?;
    let client = imap::connect((acc.host.as_str(), acc.port), &acc.host, &tls)
        .map_err(|e| anyhow!("Failed to connect to {}:{}: {}", acc.host, acc.port, e))?;

    let session = client.login(&acc.user, &acc.pass)
        .map_err(|(e, _)| anyhow!("IMAP login failed for {}: {}", acc.user, e))?;

    Ok(session)
}

/// MIME ヘッダ（RFC 2047: =?UTF-8?B?...?= など）の安全なデコード
fn decode_mime_header(raw: &[u8]) -> String {
    let s = String::from_utf8_lossy(raw);
    let decoded = scan_and_decode_rfc2047(&s);
    if !decoded.is_empty() {
        decoded
    } else {
        s.into_owned()
    }
}

/// 文字列内の =?charset?encoding?encoded_text?= または =_charset_encoding_encoded_text_= を順次デコード
fn scan_and_decode_rfc2047(text: &str) -> String {
    let mut out = String::new();
    let mut i = 0;
    let chars: Vec<char> = text.chars().collect();

    while i < chars.len() {
        if chars[i] == '=' && i + 1 < chars.len() && (chars[i + 1] == '?' || chars[i + 1] == '_') {
            let delim = chars[i + 1];
            let start = i;

            let mut delim_count = 0;
            let mut found_end = None;

            let mut j = i + 2;
            while j < chars.len() {
                if chars[j] == delim {
                    if j + 1 < chars.len() && chars[j + 1] == '=' && delim_count >= 2 {
                        found_end = Some(j + 2);
                        break;
                    }
                    delim_count += 1;
                }
                j += 1;
            }

            if let Some(end) = found_end {
                let word: String = chars[start..end].iter().collect();
                if let Ok(decoded) = decode_single_rfc2047(&word, delim) {
                    out.push_str(&decoded);
                    i = end;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }

    if out.is_empty() {
        text.to_string()
    } else {
        out
    }
}

/// 単一の RFC 2047 エンコード単語をデコード
fn decode_single_rfc2047(encoded: &str, delim: char) -> Result<String> {
    let start_marker = format!("={}", delim);
    let end_marker = format!("{}=", delim);
    let inner = encoded.trim_start_matches(&start_marker).trim_end_matches(&end_marker);
    let parts: Vec<&str> = inner.splitn(3, delim).collect();
    if parts.len() != 3 {
        return Err(anyhow!("Invalid rfc2047 syntax"));
    }
    let charset = parts[0].to_lowercase();
    let encoding = parts[1].to_uppercase();
    let text = parts[2];

    let bytes = if encoding == "B" {
        base64_lite::decode_b64(text)?
    } else if encoding == "Q" {
        decode_qp(text)?
    } else {
        return Err(anyhow!("Unsupported encoding: {}", encoding));
    };

    if charset.contains("utf-8") {
        Ok(String::from_utf8_lossy(&bytes).to_string())
    } else if charset.contains("iso-2022-jp") {
        let (cow, _, _) = encoding_rs::ISO_2022_JP.decode(&bytes);
        Ok(cow.to_string())
    } else if charset.contains("shift_jis") || charset.contains("sjis") || charset.contains("windows-31j") || charset.contains("cp932") {
        let (cow, _, _) = encoding_rs::SHIFT_JIS.decode(&bytes);
        Ok(cow.to_string())
    } else if charset.contains("euc-jp") {
        let (cow, _, _) = encoding_rs::EUC_JP.decode(&bytes);
        Ok(cow.to_string())
    } else {
        Ok(String::from_utf8_lossy(&bytes).to_string())
    }
}

mod base64_lite {
    use anyhow::Result;
    pub fn decode_b64(s: &str) -> Result<Vec<u8>> {
        let clean: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        const B64_CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = Vec::new();
        let mut buf = 0u32;
        let mut bits = 0;

        for &b in clean.as_bytes() {
            if b == b'=' {
                break;
            }
            let val = match B64_CHARS.iter().position(|&x| x == b) {
                Some(p) => p as u32,
                None => continue,
            };
            buf = (buf << 6) | val;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((buf >> bits) as u8);
            }
        }
        Ok(out)
    }
}

// -------------------------------------------------------------
// RFC 3501 IMAP Modified UTF-7 エンコード / デコード
// -------------------------------------------------------------

const MODIFIED_B64_CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+,";

fn decode_modified_b64(s: &str) -> Result<String> {
    let mut bits = 0u32;
    let mut bit_count = 0;
    let mut bytes = Vec::new();

    for &b in s.as_bytes() {
        let val = match MODIFIED_B64_CHARS.iter().position(|&x| x == b) {
            Some(p) => p as u32,
            None => continue,
        };
        bits = (bits << 6) | val;
        bit_count += 6;
        if bit_count >= 8 {
            bit_count -= 8;
            bytes.push((bits >> bit_count) as u8);
        }
    }

    if bytes.len() % 2 != 0 {
        return Err(anyhow!("Invalid UTF-16 byte length in modified UTF-7"));
    }

    let u16_vec: Vec<u16> = bytes.chunks_exact(2)
        .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
        .collect();

    let decoded = char::decode_utf16(u16_vec)
        .collect::<std::result::Result<String, _>>()
        .map_err(|e| anyhow!("UTF-16 decode error: {}", e))?;

    Ok(decoded)
}

fn encode_modified_b64(u16_slice: &[u16]) -> String {
    let mut bytes = Vec::with_capacity(u16_slice.len() * 2);
    for &val in u16_slice {
        bytes.extend_from_slice(&val.to_be_bytes());
    }

    let mut out = String::new();
    let mut bits = 0u32;
    let mut bit_count = 0;

    for &b in &bytes {
        bits = (bits << 8) | (b as u32);
        bit_count += 8;
        while bit_count >= 6 {
            bit_count -= 6;
            let idx = ((bits >> bit_count) & 0x3F) as usize;
            out.push(MODIFIED_B64_CHARS[idx] as char);
        }
    }

    if bit_count > 0 {
        let idx = ((bits << (6 - bit_count)) & 0x3F) as usize;
        out.push(MODIFIED_B64_CHARS[idx] as char);
    }

    out
}

/// IMAP modified UTF-7 文字列を通常の UTF-8 文字列にデコード
fn decode_modified_utf7(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '&' {
            if let Some(pos) = chars[i + 1..].iter().position(|&c| c == '-') {
                let end = i + 1 + pos;
                if end == i + 1 {
                    out.push('&');
                } else {
                    let b64_slice: String = chars[i + 1..end].iter().collect();
                    if let Ok(decoded) = decode_modified_b64(&b64_slice) {
                        out.push_str(&decoded);
                    } else {
                        let raw: String = chars[i..=end].iter().collect();
                        out.push_str(&raw);
                    }
                }
                i = end + 1;
                continue;
            } else {
                out.push(chars[i]);
                i += 1;
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }

    out
}

/// 通常の UTF-8 文字列を IMAP modified UTF-7 文字列にエンコード
fn encode_modified_utf7(s: &str) -> String {
    let mut out = String::new();
    let mut non_ascii_buf: Vec<u16> = Vec::new();

    for c in s.chars() {
        let code = c as u32;
        if (0x20..=0x25).contains(&code) || (0x27..=0x7E).contains(&code) {
            if !non_ascii_buf.is_empty() {
                out.push('&');
                out.push_str(&encode_modified_b64(&non_ascii_buf));
                out.push('-');
                non_ascii_buf.clear();
            }
            out.push(c);
        } else if code == 0x26 {
            if !non_ascii_buf.is_empty() {
                out.push('&');
                out.push_str(&encode_modified_b64(&non_ascii_buf));
                out.push('-');
                non_ascii_buf.clear();
            }
            out.push_str("&-");
        } else {
            let mut buf = [0u16; 2];
            for &unit in &*c.encode_utf16(&mut buf) {
                non_ascii_buf.push(unit);
            }
        }
    }

    if !non_ascii_buf.is_empty() {
        out.push('&');
        out.push_str(&encode_modified_b64(&non_ascii_buf));
        out.push('-');
    }

    out
}

fn decode_qp(s: &str) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'=' && i + 2 < bytes.len() {
            if let Ok(val) = u8::from_str_radix(std::str::from_utf8(&bytes[i+1..i+3])?, 16) {
                out.push(val);
                i += 3;
                continue;
            }
        } else if bytes[i] == b'_' {
            out.push(b' ');
            i += 1;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    Ok(out)
}

/// Windows ファイル名として使えない文字を安全に置換
fn sanitize_filename(s: &str) -> String {
    s.chars().map(|c| match c {
        '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\r' | '\n' | '\t' => '_',
        _ => c,
    }).collect()
}

/// 文字列を指定の最大文字数（char数）に安全に切り詰め（末尾に "..." を付加）
fn truncate_str(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        s.to_string()
    } else {
        let budget = max_chars.saturating_sub(3);
        let cut: String = s.chars().take(budget).collect();
        format!("{}...", cut.trim_end())
    }
}

/// ファイル名または文字列から UID（例: "[12345]" または "12345"）を抽出する
fn extract_uid(filename: &str) -> Option<u32> {
    let trimmed = filename.trim();
    if let Ok(uid) = trimmed.parse::<u32>() {
        return Some(uid);
    }
    for part in trimmed.split('[') {
        if let Some(end) = part.find(']') {
            let candidate = &part[..end];
            if let Ok(uid) = candidate.trim().parse::<u32>() {
                return Some(uid);
            }
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MailPath {
    account: Option<String>,
    folder: Option<String>,
    filename: Option<String>,
    uid: Option<u32>,
}

/// 引数のパス `<account>/<folder...>/[uid] filename` を分解
fn parse_mail_path(raw: &str) -> MailPath {
    let mut s = raw.trim().trim_matches('"').trim_matches('\'').replace('\\', "/");
    let s_lower = s.to_lowercase();

    // aux://S_auximap/ または aux://S_auximapmail/ などのプレフィックスがあれば除去
    let prefixes = [
        "aux://s_auximap/",
        "aux:/s_auximap/",
        "aux:s_auximap/",
        "aux://s_auximapmail/",
        "aux:/s_auximapmail/",
        "aux:s_auximapmail/",
        "aux://s_imapmail/",
        "aux:/s_imapmail/",
        "aux:s_imapmail/",
    ];

    let mut matched_prefix = false;
    for prefix in &prefixes {
        if let Some(pos) = s_lower.find(prefix) {
            s = s[pos + prefix.len()..].to_string();
            matched_prefix = true;
            break;
        }
    }

    if !matched_prefix {
        if let Some(pos) = s_lower.find("aux://") {
            if let Some(slash) = s[pos + 6..].find('/') {
                s = s[pos + 6 + slash + 1..].to_string();
            }
        } else if let Some(pos) = s_lower.find("aux:") {
            let after = &s[pos + 4..];
            if let Some(slash) = after.find('/') {
                s = after[slash + 1..].to_string();
            }
        }
    }

    let segments: Vec<&str> = s.split('/')
        .map(|seg| seg.trim())
        .filter(|seg| !seg.is_empty())
        .collect();

    if segments.is_empty() {
        return MailPath { account: None, folder: None, filename: None, uid: None };
    }

    let account = Some(segments[0].to_string());
    if segments.len() == 1 {
        return MailPath { account, folder: None, filename: None, uid: None };
    }

    let last = segments[segments.len() - 1];
    let uid = extract_uid(last);
    let is_file = uid.is_some() || last.to_lowercase().ends_with(".eml");

    if is_file {
        let filename = Some(last.to_string());
        if segments.len() == 2 {
            MailPath { account, folder: None, filename, uid }
        } else {
            let folder_parts = &segments[1..segments.len() - 1];
            let folder = Some(folder_parts.join("/"));
            MailPath { account, folder, filename, uid }
        }
    } else {
        let folder_parts = &segments[1..];
        let folder = Some(folder_parts.join("/"));
        MailPath { account, folder, filename: None, uid: None }
    }
}

fn is_local_path(p: &str) -> bool {
    let s = p.trim();
    if s.to_lowercase().starts_with("aux:") {
        return false;
    }
    if s.len() >= 2 && s.chars().nth(1) == Some(':') {
        return true;
    }
    if s.starts_with(r"\\") || s.starts_with("//") {
        return true;
    }
    false
}

// -------------------------------------------------------------
// 各コマンドの処理
// -------------------------------------------------------------

fn format_message_date(msg: &imap::types::Fetch, timezone: &str, date_source: &str) -> String {
    let dt_opt: Option<chrono::DateTime<chrono::FixedOffset>> = if date_source == "header" {
        msg.envelope().and_then(|env| env.date.as_deref()).and_then(|d| {
            let s = String::from_utf8_lossy(d);
            chrono::DateTime::parse_from_rfc2822(s.trim()).ok()
        }).or_else(|| msg.internal_date())
    } else {
        msg.internal_date().or_else(|| {
            msg.envelope().and_then(|env| env.date.as_deref()).and_then(|d| {
                let s = String::from_utf8_lossy(d);
                chrono::DateTime::parse_from_rfc2822(s.trim()).ok()
            })
        })
    };

    if let Some(dt) = dt_opt {
        match timezone {
            "server" | "raw" => dt.format("%Y-%m-%d %H:%M:%S").to_string(),
            "utc" => dt.naive_utc().format("%Y-%m-%d %H:%M:%S").to_string(),
            _ => dt.with_timezone(&Local).format("%Y-%m-%d %H:%M:%S").to_string(),
        }
    } else {
        Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
    }
}

fn write_message_entry<W: Write>(writer: &mut W, msg: &imap::types::Fetch, acc: &AccountConfig) -> Result<()> {
    let uid = msg.uid.unwrap_or(0);
    let size = msg.size.unwrap_or(0);
    let date_str = format_message_date(msg, &acc.timezone, &acc.date_source);

    let mut from_name = String::new();
    let mut from_addr = String::new();
    let mut subject_str = String::from("(no subject)");

    if let Some(env) = msg.envelope() {
        if let Some(from_list) = &env.from {
            if let Some(first_from) = from_list.first() {
                from_name = first_from.name.as_deref().map(decode_mime_header).unwrap_or_default();
                let mailbox = first_from.mailbox.as_deref().map(|b| String::from_utf8_lossy(b).to_string()).unwrap_or_default();
                let host = first_from.host.as_deref().map(|b| String::from_utf8_lossy(b).to_string()).unwrap_or_default();

                if !mailbox.is_empty() && !host.is_empty() {
                    from_addr = format!("{}@{}", mailbox, host);
                }
            }
        }
        if let Some(subj) = &env.subject {
            subject_str = decode_mime_header(subj);
        }
    }

    let from_combined = if !from_name.is_empty() && !from_addr.is_empty() {
        if from_name.eq_ignore_ascii_case(&from_addr) {
            from_addr
        } else {
            format!("{} ({})", from_name, from_addr)
        }
    } else if !from_name.is_empty() {
        from_name
    } else if !from_addr.is_empty() {
        from_addr
    } else {
        String::from("unknown")
    };

    let uid_part = format!("[{}] ", uid);
    let clean_from = truncate_str(&sanitize_filename(&from_combined), 45);
    let suffix = format!(" - {}.eml", clean_from);
    let max_subject_len = 160usize.saturating_sub(uid_part.chars().count() + suffix.chars().count());
    let clean_subject = truncate_str(&sanitize_filename(&subject_str), std::cmp::max(max_subject_len, 30));

    let entry_name = format!("{}{}{}", uid_part, clean_subject, suffix);
    write!(writer, "\"{}\",A:H20,S:{},W:{}\r\n", entry_name, size, date_str)?;
    Ok(())
}

/// メッセージシーケンス番号の取得範囲（start_seq, end_seq）を計算
/// total: フォルダ内総メール数
/// limit: 取得件数
/// offset: 最新からスキップする件数
/// 戻り値: Some((start_seq, end_seq)) または 取得対象がない場合は None
fn calculate_seq_range(total: u32, limit: u32, offset: u32) -> Option<(u32, u32)> {
    if total == 0 || offset >= total || limit == 0 {
        return None;
    }
    let end_seq = total - offset;
    let start_seq = if end_seq > limit {
        end_seq - limit + 1
    } else {
        1
    };
    Some((start_seq, end_seq))
}

/// 一覧取得 (list)
fn cmd_list(
    target_path: &str,
    output_file: &str,
    cli_chunk: Option<u32>,
    cli_offset: Option<u32>,
    cli_limit: Option<u32>,
    cli_pass: Option<&str>,
    cli_user: Option<&str>,
) -> Result<()> {
    let path_info = parse_mail_path(target_path);

    let out_path = Path::new(output_file);
    let mut writer = BufWriter::new(File::create(out_path).context("Failed to create output file")?);

    // 1. ルート階層（アカウント一覧）の場合:
    // パスワード解決は一切不要！load_config_raw でアカウント名のみを取得して出力
    if path_info.account.is_none() {
        let (accounts, _, _, _) = load_config_raw()?;
        write!(writer, ";ListFile\r\n")?;
        for acc in &accounts {
            write!(writer, "\"{}\",A:H10,W:{}\r\n", acc.name, Local::now().format("%Y-%m-%d %H:%M:%S"))?;
        }
        return Ok(());
    }

    // 2. アカウント指定がある場合:
    // そのアカウントのみを対象にパスワード解決
    let acc_name = path_info.account.unwrap();
    let (accounts, _) = match load_config(cli_pass, cli_user, Some(&acc_name)) {
        Ok(v) => v,
        Err(e) => {
            // エラー時でも空のリストファイルを出力して PPc の無限リトライを防止
            let _ = write!(writer, ";ListFile\r\n");
            let _ = writer.flush();
            return Err(e);
        }
    };

    let acc = accounts.iter().find(|a| a.name.eq_ignore_ascii_case(&acc_name))
        .ok_or_else(|| anyhow!("Account '{}' not found in auximap.ini", acc_name))?;

    let mut session = match connect_imap(acc) {
        Ok(s) => s,
        Err(e) => {
            let _ = write!(writer, ";ListFile\r\n");
            let _ = writer.flush();
            return Err(e);
        }
    };

    if path_info.folder.is_none() || path_info.folder.as_deref() == Some("") {
        let mailboxes = session.list(Some(""), Some("*")).context("Failed to list IMAP mailboxes")?;
        write!(writer, ";ListFile\r\n")?;
        for mb in mailboxes.iter() {
            let name = mb.name();
            if name.is_empty() {
                continue;
            }
            let decoded_name = decode_modified_utf7(name);
            write!(writer, "\"{}\",A:H10,W:{}\r\n", decoded_name, Local::now().format("%Y-%m-%d %H:%M:%S"))?;
        }
        return Ok(());
    }

    let folder_name = path_info.folder.unwrap();
    let imap_folder = encode_modified_utf7(&folder_name);
    let mailbox = session.select(&imap_folder)
        .map_err(|e| anyhow!("Failed to select mailbox '{}': {}", folder_name, e))?;

    let total = mailbox.exists;
    write!(writer, ";ListFile\r\n")?;

    let limit = cli_limit.unwrap_or(acc.limit);
    let offset = cli_offset.unwrap_or(acc.offset);

    let (start_seq, end_seq) = match calculate_seq_range(total, limit, offset) {
        Some(range) => range,
        None => {
            writer.flush().ok();
            return Ok(());
        }
    };

    let effective_chunk = cli_chunk.or_else(|| if acc.chunk_size > 0 { Some(acc.chunk_size) } else { None });

    if let Some(chunk_size) = effective_chunk.filter(|&c| c > 0) {
        log_msg(&format!("List: Fetching {} messages (seq {}..={}) in chunks of {}", end_seq - start_seq + 1, start_seq, end_seq, chunk_size));
        let mut cur_end = end_seq;

        while cur_end >= start_seq {
            let cur_start = if cur_end >= chunk_size {
                std::cmp::max(start_seq, cur_end - chunk_size + 1)
            } else {
                start_seq
            };

            let range = format!("{}:{}", cur_start, cur_end);
            match session.fetch(&range, "(UID RFC822.SIZE INTERNALDATE ENVELOPE)") {
                Ok(messages) => {
                    for msg in messages.iter().rev() {
                        let _ = write_message_entry(&mut writer, msg, acc);
                    }
                }
                Err(e) => {
                    log_msg(&format!("Failed to fetch range {}, error: {:?}. Retrying individually...", range, e));
                    for seq in (cur_start..=cur_end).rev() {
                        if let Ok(msgs) = session.fetch(seq.to_string(), "(UID RFC822.SIZE INTERNALDATE ENVELOPE)") {
                            for msg in msgs.iter() {
                                let _ = write_message_entry(&mut writer, msg, acc);
                            }
                        } else {
                            log_msg(&format!("Skipping unparseable message seq {}", seq));
                        }
                    }
                }
            }

            if cur_start <= start_seq {
                break;
            }
            cur_end = cur_start - 1;
        }
    } else {
        let range = format!("{}:{}", start_seq, end_seq);
        log_msg(&format!("List: Fetching range {} in bulk (single request)", range));
        let messages = session.fetch(&range, "(UID RFC822.SIZE INTERNALDATE ENVELOPE)")
            .context("Failed to fetch message headers in bulk")?;

        for msg in messages.iter().rev() {
            let _ = write_message_entry(&mut writer, msg, acc);
        }
    }

    writer.flush().ok();
    Ok(())
}

fn format_bytes_human(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// HTMLタグを除去してプレーンテキストに変換
fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut current_tag = String::new();
    let mut in_script_or_style = false;

    for ch in html.chars() {
        if in_tag {
            if ch == '>' {
                in_tag = false;
                let tag_lower = current_tag.trim().to_lowercase();

                if tag_lower.starts_with("script") || tag_lower.starts_with("style") {
                    in_script_or_style = true;
                } else if tag_lower.starts_with("/script") || tag_lower.starts_with("/style") {
                    in_script_or_style = false;
                } else if !in_script_or_style {
                    if tag_lower.starts_with("br")
                        || tag_lower.starts_with("/p")
                        || tag_lower.starts_with("/div")
                        || tag_lower.starts_with("/tr")
                        || tag_lower.starts_with("/h")
                        || tag_lower.starts_with("li")
                    {
                        result.push('\n');
                    }
                }
                current_tag.clear();
            } else {
                current_tag.push(ch);
            }
        } else if ch == '<' {
            in_tag = true;
            current_tag.clear();
        } else if !in_script_or_style {
            result.push(ch);
        }
    }

    decode_html_entities(&result)
}

/// HTML 実体参照（エンティティ）のデコード
fn decode_html_entities(text: &str) -> String {
    let s = text.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&copy;", "©")
        .replace("&reg;", "®");

    let mut clean = String::new();
    let mut consecutive_newlines = 0;
    for ch in s.chars() {
        if ch == '\r' {
            continue;
        }
        if ch == '\n' {
            consecutive_newlines += 1;
            if consecutive_newlines <= 2 {
                clean.push('\n');
            }
        } else {
            consecutive_newlines = 0;
            clean.push(ch);
        }
    }
    clean.trim().to_string()
}

/// マルチパート構造を再帰的に走査し、本文と添付ファイルを抽出
fn extract_mail_parts(
    part: &mailparse::ParsedMail,
    plain_texts: &mut Vec<String>,
    html_texts: &mut Vec<String>,
    attachments: &mut Vec<String>,
) {
    if !part.subparts.is_empty() {
        for sub in &part.subparts {
            extract_mail_parts(sub, plain_texts, html_texts, attachments);
        }
        return;
    }

    let disposition = part.get_content_disposition();
    let is_attachment = disposition.disposition == mailparse::DispositionType::Attachment;
    let filename_param = disposition.params.get("filename")
        .or_else(|| part.ctype.params.get("name"));

    if is_attachment || (filename_param.is_some() && part.ctype.mimetype != "text/plain" && part.ctype.mimetype != "text/html") {
        let raw_name = filename_param.cloned().unwrap_or_else(|| "attachment.bin".to_string());
        let decoded_name = scan_and_decode_rfc2047(&raw_name);
        let size_bytes = part.get_body_raw().map(|b| b.len()).unwrap_or(0);
        let size_str = format_bytes_human(size_bytes);
        attachments.push(format!("{} ({})", decoded_name, size_str));
        return;
    }

    if part.ctype.mimetype == "text/plain" {
        if let Ok(body) = part.get_body() {
            plain_texts.push(body);
        }
    } else if part.ctype.mimetype == "text/html" {
        if let Ok(body) = part.get_body() {
            html_texts.push(body);
        }
    }
}

/// テンプレートファイルの読み込み
fn load_template(view_conf: &ViewConfig, exe_dir: &Path) -> String {
    if let Some(ref rel_or_abs) = view_conf.template_file {
        let path = if Path::new(rel_or_abs).is_absolute() {
            PathBuf::from(rel_or_abs)
        } else {
            exe_dir.join(rel_or_abs)
        };
        if let Ok(content) = fs::read_to_string(&path) {
            return content;
        }
    }

    let default_file = exe_dir.join("view_template.txt");
    if let Ok(content) = fs::read_to_string(&default_file) {
        return content;
    }

    if let Some(ref inline) = view_conf.template_inline {
        if !inline.is_empty() {
            return inline.clone();
        }
    }

    "From   : {from}\nTo     : {to}\nCc     : {cc}\nDate   : {date}\nSubject: {subject}\nAttach : {attachments}\n--------------------------------------------------------------------------------\n\n{body}".to_string()
}

/// テンプレートにプレースホルダを適用して本文を整形
fn apply_template(
    template: &str,
    from: &str,
    to: &str,
    cc: &str,
    date: &str,
    subject: &str,
    attachments: &str,
    body: &str,
) -> String {
    let mut lines = Vec::new();
    for line in template.lines() {
        if cc.is_empty() && line.contains("{cc}") {
            continue;
        }
        if attachments.is_empty() && line.contains("{attachments}") {
            continue;
        }
        let mut rendered = line.to_string();
        rendered = rendered.replace("{from}", from);
        rendered = rendered.replace("{to}", to);
        rendered = rendered.replace("{cc}", cc);
        rendered = rendered.replace("{date}", date);
        rendered = rendered.replace("{subject}", subject);
        rendered = rendered.replace("{attachments}", attachments);
        rendered = rendered.replace("{body}", body);
        lines.push(rendered);
    }
    lines.join("\r\n")
}

/// 生メールデータをパースし整形済み文字列を生成
fn format_mail_content(
    raw_bytes: &[u8],
    timezone_setting: &str,
    view_conf: &ViewConfig,
    exe_dir: &Path,
) -> Result<String> {
    let mail = mailparse::parse_mail(raw_bytes)
        .context("Failed to parse MIME email structure")?;

    let subject_raw = mail.headers.get_first_value("Subject").unwrap_or_default();
    let subject = scan_and_decode_rfc2047(&subject_raw);

    let from_raw = mail.headers.get_first_value("From").unwrap_or_default();
    let from = scan_and_decode_rfc2047(&from_raw);

    let to_raw = mail.headers.get_first_value("To").unwrap_or_default();
    let to = scan_and_decode_rfc2047(&to_raw);

    let cc_raw = mail.headers.get_first_value("Cc").unwrap_or_default();
    let cc = scan_and_decode_rfc2047(&cc_raw);

    let date_raw = mail.headers.get_first_value("Date").unwrap_or_default();
    let date = if !date_raw.is_empty() {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc2822(date_raw.trim()) {
            match timezone_setting {
                "server" | "raw" => dt.format("%Y-%m-%d %H:%M:%S").to_string(),
                "utc" => dt.naive_utc().format("%Y-%m-%d %H:%M:%S").to_string(),
                _ => dt.with_timezone(&Local).format("%Y-%m-%d %H:%M:%S").to_string(),
            }
        } else {
            date_raw
        }
    } else {
        String::new()
    };

    let mut plain_texts = Vec::new();
    let mut html_texts = Vec::new();
    let mut attachments = Vec::new();

    extract_mail_parts(&mail, &mut plain_texts, &mut html_texts, &mut attachments);

    let body = if !plain_texts.is_empty() {
        plain_texts.join("\r\n\r\n")
    } else if !html_texts.is_empty() {
        let combined_html = html_texts.join("\r\n\r\n");
        if view_conf.html_policy.eq_ignore_ascii_case("raw") {
            combined_html
        } else {
            strip_html_tags(&combined_html)
        }
    } else {
        "(No text content)".to_string()
    };

    let attachments_str = attachments.join(", ");
    let template = load_template(view_conf, exe_dir);

    let rendered = apply_template(
        &template,
        &from,
        &to,
        &cc,
        &date,
        &subject,
        &attachments_str,
        &body,
    );

    Ok(rendered)
}

/// メール本文閲覧 (read / view)
fn cmd_read(src: &MailPath, dest_arg: Option<&str>, cli_pass: Option<&str>, cli_user: Option<&str>) -> Result<()> {
    let acc_name = src.account.as_deref().ok_or_else(|| anyhow!("Source account not specified in source: {:?}", src))?;
    let (accounts, ini_path) = load_config(cli_pass, cli_user, Some(acc_name))?;
    let view_conf = load_view_config(&ini_path);
    let exe_dir = ini_path.parent().unwrap_or_else(|| Path::new("."));

    let folder_name = src.folder.as_deref().ok_or_else(|| anyhow!("Source folder not specified in source: {:?}", src))?;
    let uid = src.uid.ok_or_else(|| anyhow!("UID not specified in source: {:?}", src))?;

    let acc = accounts.iter().find(|a| a.name.eq_ignore_ascii_case(acc_name))
        .ok_or_else(|| anyhow!("Account '{}' not found", acc_name))?;

    let mut session = connect_imap(acc)?;
    let imap_folder = encode_modified_utf7(folder_name);
    session.select(&imap_folder).context("Failed to select folder")?;

    let messages = session.uid_fetch(uid.to_string(), "RFC822")
        .context("Failed to fetch message RFC822")?;

    let msg = messages.iter().next()
        .ok_or_else(|| anyhow!("Message with UID {} not found on server", uid))?;

    let raw_bytes = msg.body().ok_or_else(|| anyhow!("Message body is empty for UID {}", uid))?;

    let formatted = format_mail_content(raw_bytes, &acc.timezone, &view_conf, exe_dir)?;

    if let Some(dest_path) = dest_arg {
        if dest_path != "-" {
            let mut dest = PathBuf::from(dest_path);
            if dest.is_dir() || dest_path.ends_with('\\') || dest_path.ends_with('/') {
                let fname = format!("mail_{}.txt", uid);
                dest = dest.join(fname);
            }
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).ok();
            }
            let mut file = File::create(&dest).context("Failed to create destination file")?;
            file.write_all(b"\xEF\xBB\xBF")?; // UTF-8 BOM
            file.write_all(formatted.as_bytes())?;
            return Ok(());
        }
    }

    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    handle.write_all(formatted.as_bytes())?;
    handle.write_all(b"\n")?;
    handle.flush()?;

    Ok(())
}

/// メール取得 (get) -> 一時ファイルまたはローカルファイルへ保存
fn cmd_get(src: &MailPath, dest_path: &str, cli_pass: Option<&str>, cli_user: Option<&str>) -> Result<()> {
    let acc_name = src.account.as_deref().ok_or_else(|| anyhow!("Source account not specified"))?;
    let (accounts, _) = load_config(cli_pass, cli_user, Some(acc_name))?;
    let folder_name = src.folder.as_deref().ok_or_else(|| anyhow!("Source folder not specified"))?;
    let uid = src.uid.ok_or_else(|| anyhow!("UID not specified in source: {:?}", src))?;
    let filename = src.filename.as_deref().unwrap_or("mail.eml");

    let acc = accounts.iter().find(|a| a.name.eq_ignore_ascii_case(acc_name))
        .ok_or_else(|| anyhow!("Account '{}' not found", acc_name))?;

    let mut session = connect_imap(acc)?;
    let imap_folder = encode_modified_utf7(folder_name);
    session.select(&imap_folder).context("Failed to select folder")?;

    let messages = session.uid_fetch(uid.to_string(), "RFC822")
        .context("Failed to fetch message body")?;

    let msg = messages.iter().next()
        .ok_or_else(|| anyhow!("Message with UID {} not found on server", uid))?;

    let body = msg.body().ok_or_else(|| anyhow!("Message body is empty for UID {}", uid))?;

    let mut dest = PathBuf::from(dest_path);
    if dest.is_dir() || dest_path.ends_with('\\') || dest_path.ends_with('/') {
        dest = dest.join(filename);
    }

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).ok();
    }

    let mut file = File::create(&dest).context("Failed to create destination file")?;
    file.write_all(body).context("Failed to write email data")?;

    Ok(())
}

/// メール複写 (copy) -> 相手先フォルダへ複製（元の場所にも残る）
fn cmd_copy(src: &MailPath, dest_arg: &str, cli_pass: Option<&str>, cli_user: Option<&str>) -> Result<()> {
    let src_acc_name = src.account.as_deref().ok_or_else(|| anyhow!("Source account not specified"))?;
    let (accounts, _) = load_config(cli_pass, cli_user, Some(src_acc_name))?;
    let src_folder = src.folder.as_deref().ok_or_else(|| anyhow!("Source folder not specified"))?;
    let uid = src.uid.ok_or_else(|| anyhow!("UID not specified in source: {:?}", src))?;

    // ローカルパス（C:\... など）の場合はローカル保存として処理
    if is_local_path(dest_arg) {
        return cmd_get(src, dest_arg, cli_pass, cli_user);
    }

    let dest = parse_mail_path(dest_arg);
    let dest_acc_name = dest.account.as_deref().unwrap_or(src_acc_name);
    let dest_folder = dest.folder.as_deref()
        .ok_or_else(|| anyhow!("Destination folder not specified in '{}'", dest_arg))?;

    if src_acc_name.eq_ignore_ascii_case(dest_acc_name) {
        let acc = accounts.iter().find(|a| a.name.eq_ignore_ascii_case(src_acc_name))
            .ok_or_else(|| anyhow!("Account '{}' not found", src_acc_name))?;

        let mut session = connect_imap(acc)?;
        let src_imap = encode_modified_utf7(src_folder);
        let dest_imap = encode_modified_utf7(dest_folder);
        session.select(&src_imap).context("Failed to select source folder")?;
        session.uid_copy(uid.to_string(), &dest_imap)
            .context(format!("Failed to copy message to folder '{}'", dest_folder))?;
    } else {
        let src_acc = accounts.iter().find(|a| a.name.eq_ignore_ascii_case(src_acc_name))
            .ok_or_else(|| anyhow!("Source account '{}' not found", src_acc_name))?;
        let dest_acc = accounts.iter().find(|a| a.name.eq_ignore_ascii_case(dest_acc_name))
            .ok_or_else(|| anyhow!("Destination account '{}' not found", dest_acc_name))?;

        let mut src_session = connect_imap(src_acc)?;
        let src_imap = encode_modified_utf7(src_folder);
        let dest_imap = encode_modified_utf7(dest_folder);
        src_session.select(&src_imap)?;
        let messages = src_session.uid_fetch(uid.to_string(), "RFC822")?;
        let msg = messages.iter().next().ok_or_else(|| anyhow!("Message not found"))?;
        let body = msg.body().ok_or_else(|| anyhow!("Message body empty"))?;

        let mut dest_session = connect_imap(dest_acc)?;
        dest_session.append(&dest_imap, body)?;
    }

    Ok(())
}

/// 指定メールを移動元フォルダから直接完全抹消（\Deleted + EXPUNGE）
fn cmd_expunge(src: &MailPath, cli_pass: Option<&str>, cli_user: Option<&str>) -> Result<()> {
    let acc_name = src.account.as_deref().ok_or_else(|| anyhow!("Account not specified"))?;
    let (accounts, _) = load_config(cli_pass, cli_user, Some(acc_name))?;
    let folder_name = src.folder.as_deref().ok_or_else(|| anyhow!("Folder not specified"))?;
    let uid = src.uid.ok_or_else(|| anyhow!("UID not specified in source: {:?}", src))?;

    let acc = accounts.iter().find(|a| a.name.eq_ignore_ascii_case(acc_name))
        .ok_or_else(|| anyhow!("Account '{}' not found", acc_name))?;

    let mut session = connect_imap(acc)?;
    let imap_folder = encode_modified_utf7(folder_name);
    session.select(&imap_folder).context("Failed to select folder")?;
    session.uid_store(uid.to_string(), "+FLAGS (\\Deleted)")?;
    session.expunge()?;

    Ok(())
}

/// メール移動 (move) -> 相手先フォルダへ複製し、元の場所からは削除
fn cmd_move(src: &MailPath, dest_arg: &str, cli_pass: Option<&str>, cli_user: Option<&str>) -> Result<()> {
    // 1. まず複写（またはローカル取得）を実行
    cmd_copy(src, dest_arg, cli_pass, cli_user)?;

    // 2. 複写成功後、移動元から削除
    // 移動先がローカルの場合はゴミ箱移動、IMAP フォルダ間移動の場合は移動元から直接 EXPUNGE（消去）
    if is_local_path(dest_arg) {
        cmd_delete(src, cli_pass, cli_user)?;
    } else {
        cmd_expunge(src, cli_pass, cli_user)?;
    }

    Ok(())
}

static DEBUG_MODE: AtomicBool = AtomicBool::new(false);

fn set_debug_mode(enabled: bool) {
    DEBUG_MODE.store(enabled, Ordering::Relaxed);
}

fn log_msg(msg: &str) {
    if !DEBUG_MODE.load(Ordering::Relaxed) {
        return;
    }
    eprintln!("{}", msg);
    let now = Local::now().format("%Y-%m-%d %H:%M:%S");
    let line = format!("[{}] {}\r\n", now, msg);

    if let Ok(exe) = env::current_exe() {
        let exe_log = exe.parent().unwrap_or_else(|| Path::new(".")).join("auximap.log");
        if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&exe_log) {
            let _ = f.write_all(line.as_bytes());
            return;
        }
    }

    let temp_log = env::temp_dir().join("auximap.log");
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&temp_log) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// 複数メール削除 (delete) -> Trash フォルダへ一括移動または \Deleted フラグ
fn cmd_delete_multi(acc_name: &str, folder_name: &str, uids: &[u32], cli_pass: Option<&str>, cli_user: Option<&str>) -> Result<()> {
    if uids.is_empty() {
        return Ok(());
    }

    let (accounts, _) = load_config(cli_pass, cli_user, Some(acc_name))?;
    let acc = accounts.iter().find(|a| a.name.eq_ignore_ascii_case(acc_name))
        .ok_or_else(|| anyhow!("Account '{}' not found in auximap.ini", acc_name))?;

    let mut session = connect_imap(acc)?;
    let imap_folder = encode_modified_utf7(folder_name);
    session.select(&imap_folder).context("Failed to select folder")?;

    // サーバー上の全フォルダ一覧を取得してゴミ箱フォルダを特定
    let mut candidate_trash_folders: Vec<String> = Vec::new();

    // 1. 設定ファイル指定のゴミ箱名
    if !acc.trash_folder.is_empty() {
        candidate_trash_folders.push(acc.trash_folder.clone());
    }

    // 2. サーバー上のフォルダからゴミ箱らしきものを自動検出
    if let Ok(mailboxes) = session.list(Some(""), Some("*")) {
        for mb in mailboxes.iter() {
            let decoded = decode_modified_utf7(mb.name());
            let lower = decoded.to_lowercase();
            let is_trash_attr = mb.attributes().iter().any(|a| format!("{:?}", a).to_lowercase().contains("trash"));
            if is_trash_attr
                || lower.ends_with("trash")
                || lower.contains("ごみ箱")
                || lower.contains("ゴミ箱")
                || lower.contains("deleted")
            {
                if !candidate_trash_folders.iter().any(|c| c.eq_ignore_ascii_case(&decoded)) {
                    candidate_trash_folders.push(decoded);
                }
            }
        }
    }

    // 3. 一般的なフォールバック候補
    for fallback in &["INBOX.Trash", "Trash", "[Gmail]/ゴミ箱", "[Gmail]/Trash", "Deleted Items", "INBOX.ごみ箱"] {
        if !candidate_trash_folders.iter().any(|c| c.eq_ignore_ascii_case(fallback)) {
            candidate_trash_folders.push(fallback.to_string());
        }
    }

    let uid_set = uids.iter().map(|u| u.to_string()).collect::<Vec<_>>().join(",");
    log_msg(&format!("Delete UIDs [{}] from folder '{}'. Trash candidates: {:?}", uid_set, folder_name, candidate_trash_folders));

    // すでにゴミ箱にいる場合は完全消去
    let is_already_in_trash = candidate_trash_folders.iter().any(|c| {
        c.eq_ignore_ascii_case(folder_name) || encode_modified_utf7(c).eq_ignore_ascii_case(&imap_folder)
    });

    if is_already_in_trash {
        log_msg(&format!("UIDs [{}] are already in trash. Expunging permanently.", uid_set));
        session.uid_store(&uid_set, "+FLAGS (\\Deleted)")?;
        session.expunge()?;
        return Ok(());
    }

    // ゴミ箱候補へ順次 UID COPY を試みる
    let mut copied = false;
    let mut last_err = None;

    for trash in &candidate_trash_folders {
        let trash_encoded = encode_modified_utf7(trash);
        if trash_encoded.eq_ignore_ascii_case(&imap_folder) {
            continue;
        }

        match session.uid_copy(&uid_set, &trash_encoded) {
            Ok(_) => {
                log_msg(&format!("UIDs [{}] copied to trash '{}'", uid_set, trash));
                copied = true;
                break;
            }
            Err(e) => {
                last_err = Some(e);
            }
        }
    }

    if copied {
        session.uid_store(&uid_set, "+FLAGS (\\Deleted)")?;
        session.expunge()?;
        log_msg(&format!("UIDs [{}] successfully moved to trash", uid_set));
        Ok(())
    } else {
        let err_msg = format!(
            "Failed to find or copy to trash folder. Messages are preserved to prevent data loss. Tried candidates: {:?}, Last error: {:?}",
            candidate_trash_folders, last_err
        );
        log_msg(&format!("SAFETY ABORT: {}", err_msg));
        Err(anyhow!(err_msg))
    }
}

/// 単一メール削除 (delete) -> cmd_delete_multi を呼び出す
fn cmd_delete(src: &MailPath, cli_pass: Option<&str>, cli_user: Option<&str>) -> Result<()> {
    let acc_name = src.account.as_deref().ok_or_else(|| anyhow!("Account not specified in: {:?}", src))?;
    let folder_name = src.folder.as_deref().ok_or_else(|| anyhow!("Folder not specified in: {:?}", src))?;
    let uid = src.uid.ok_or_else(|| anyhow!("UID not specified in source: {:?}", src))?;

    cmd_delete_multi(acc_name, folder_name, &[uid], cli_pass, cli_user)
}

/// フォルダ作成 (makedir) - 複数サーバー対応の自動フォールバック付き
fn cmd_makedir(target_path: &str, cli_pass: Option<&str>, cli_user: Option<&str>) -> Result<()> {
    let path = parse_mail_path(target_path);
    let acc_name = path.account.ok_or_else(|| anyhow!("Account not specified"))?;
    let (accounts, _) = load_config(cli_pass, cli_user, Some(&acc_name))?;
    let folder_name = path.folder.ok_or_else(|| anyhow!("Folder not specified"))?;

    let acc = accounts.iter().find(|a| a.name.eq_ignore_ascii_case(&acc_name))
        .ok_or_else(|| anyhow!("Account '{}' not found", acc_name))?;

    let mut session = connect_imap(acc)?;

    // 1. そのまま作成を試みる（Gmail, Yahoo, Outlook などの標準サーバーはここで即成功）
    let imap_folder = encode_modified_utf7(&folder_name);
    match session.create(&imap_folder) {
        Ok(_) => {
            log_msg(&format!("Mailbox '{}' created successfully", folder_name));
            return Ok(());
        }
        Err(e) => {
            log_msg(&format!("Direct create '{}' failed ({:?}), trying server namespace fallbacks...", folder_name, e));
        }
    }

    // 2. フォールバック試行（さくら等の Dovecot/Courier-IMAP ではルート直下が禁止され "INBOX." 配下が必須）
    let mut fallbacks = Vec::new();

    // "INBOX/" の場合は区切り文字を "." に変換
    if folder_name.to_uppercase().starts_with("INBOX/") {
        fallbacks.push(format!("INBOX.{}", &folder_name[6..]));
    }

    // "INBOX." で始まっていない場合は "INBOX." および "INBOX/" を付与して試行
    if !folder_name.to_uppercase().starts_with("INBOX.") && !folder_name.to_uppercase().starts_with("INBOX/") {
        fallbacks.push(format!("INBOX.{}", folder_name));
        fallbacks.push(format!("INBOX/{}", folder_name));
    }

    let mut last_err = None;
    for cand in fallbacks {
        let cand_encoded = encode_modified_utf7(&cand);
        match session.create(&cand_encoded) {
            Ok(_) => {
                log_msg(&format!("Mailbox '{}' created successfully via fallback", cand));
                return Ok(());
            }
            Err(e) => {
                last_err = Some(e);
            }
        }
    }

    Err(anyhow!("Failed to create mailbox '{}': {:?}", folder_name, last_err))
}

/// フォルダ削除 (deldir) - 自動フォールバック付き
fn cmd_deldir(target_path: &str, cli_pass: Option<&str>, cli_user: Option<&str>) -> Result<()> {
    let path = parse_mail_path(target_path);
    let acc_name = path.account.ok_or_else(|| anyhow!("Account not specified"))?;
    let (accounts, _) = load_config(cli_pass, cli_user, Some(&acc_name))?;
    let folder_name = path.folder.ok_or_else(|| anyhow!("Folder not specified"))?;

    let acc = accounts.iter().find(|a| a.name.eq_ignore_ascii_case(&acc_name))
        .ok_or_else(|| anyhow!("Account '{}' not found", acc_name))?;

    let mut session = connect_imap(acc)?;

    // 1. そのまま削除を試みる
    let imap_folder = encode_modified_utf7(&folder_name);
    if session.delete(&imap_folder).is_ok() {
        log_msg(&format!("Mailbox '{}' deleted successfully", folder_name));
        return Ok(());
    }

    // 2. フォールバック試行
    let mut fallbacks = Vec::new();
    if folder_name.to_uppercase().starts_with("INBOX/") {
        fallbacks.push(format!("INBOX.{}", &folder_name[6..]));
    }
    if !folder_name.to_uppercase().starts_with("INBOX.") && !folder_name.to_uppercase().starts_with("INBOX/") {
        fallbacks.push(format!("INBOX.{}", folder_name));
        fallbacks.push(format!("INBOX/{}", folder_name));
    }

    for cand in fallbacks {
        let cand_encoded = encode_modified_utf7(&cand);
        if session.delete(&cand_encoded).is_ok() {
            log_msg(&format!("Mailbox '{}' deleted successfully via fallback", cand));
            return Ok(());
        }
    }

    Err(anyhow!("Failed to delete mailbox '{}'", folder_name))
}

/// フォルダ名変更 (rename) - フォルダのみ対象
fn cmd_rename(src_path: &str, dest_path: &str, cli_pass: Option<&str>, cli_user: Option<&str>) -> Result<()> {
    let src = parse_mail_path(src_path);
    if src.filename.is_some() || src.uid.is_some() {
        return Err(anyhow!("Email messages cannot be renamed. Only folders can be renamed."));
    }

    let acc_name = src.account.ok_or_else(|| anyhow!("Account not specified"))?;
    let src_folder = src.folder.ok_or_else(|| anyhow!("Source folder not specified"))?;

    let upper_src = src_folder.to_uppercase();
    if upper_src == "INBOX" || upper_src == "TRASH" || upper_src == "SENT" || upper_src == "DRAFTS" || upper_src == "JUNK" || upper_src == "SPAM" {
        return Err(anyhow!("Cannot rename system folder: {}", src_folder));
    }

    let dest = parse_mail_path(dest_path);
    let dest_folder = if let Some(df) = dest.folder {
        df
    } else {
        let trimmed = dest_path.trim().trim_matches('"').trim_matches('\'').trim_matches(['/', '\\']);
        if trimmed.is_empty() {
            return Err(anyhow!("Destination folder name cannot be empty"));
        }
        if let Some(pos) = src_folder.rfind('/') {
            format!("{}/{}", &src_folder[..pos], trimmed)
        } else if let Some(pos) = src_folder.rfind('.') {
            format!("{}.{}", &src_folder[..pos], trimmed)
        } else {
            trimmed.to_string()
        }
    };

    if dest_folder.is_empty() || src_folder.eq_ignore_ascii_case(&dest_folder) {
        return Ok(());
    }

    let (accounts, _) = load_config(cli_pass, cli_user, Some(&acc_name))?;
    let acc = accounts.iter().find(|a| a.name.eq_ignore_ascii_case(&acc_name))
        .ok_or_else(|| anyhow!("Account '{}' not found", acc_name))?;

    let mut session = connect_imap(acc)?;

    // 1. 直接 rename 試行
    let src_encoded = encode_modified_utf7(&src_folder);
    let dest_encoded = encode_modified_utf7(&dest_folder);
    if session.rename(&src_encoded, &dest_encoded).is_ok() {
        log_msg(&format!("Mailbox renamed successfully: '{}' -> '{}'", src_folder, dest_folder));
        return Ok(());
    }

    // 2. サーバー仕様差（INBOX.プレフィックス等）へのフォールバック試行
    let mut fallbacks = Vec::new();
    if src_folder.to_uppercase().starts_with("INBOX/") {
        let from_cand = format!("INBOX.{}", &src_folder[6..]);
        let to_cand = if dest_folder.to_uppercase().starts_with("INBOX/") {
            format!("INBOX.{}", &dest_folder[6..])
        } else {
            format!("INBOX.{}", dest_folder)
        };
        fallbacks.push((from_cand, to_cand));
    }
    if !src_folder.to_uppercase().starts_with("INBOX.") && !src_folder.to_uppercase().starts_with("INBOX/") {
        fallbacks.push((format!("INBOX.{}", src_folder), format!("INBOX.{}", dest_folder)));
        fallbacks.push((format!("INBOX/{}", src_folder), format!("INBOX/{}", dest_folder)));
    }

    for (cand_from, cand_to) in fallbacks {
        let from_enc = encode_modified_utf7(&cand_from);
        let to_enc = encode_modified_utf7(&cand_to);
        if session.rename(&from_enc, &to_enc).is_ok() {
            log_msg(&format!("Mailbox renamed successfully via fallback: '{}' -> '{}'", cand_from, cand_to));
            return Ok(());
        }
    }

    Err(anyhow!("Failed to rename mailbox '{}' to '{}'", src_folder, dest_folder))
}

fn is_opt(arg: &str, names: &[&str]) -> bool {
    let s = arg.trim();
    let without_prefix = if let Some(r) = s.strip_prefix("--") {
        r
    } else if let Some(r) = s.strip_prefix('-') {
        r
    } else if let Some(r) = s.strip_prefix('/') {
        r
    } else {
        return false;
    };
    names.iter().any(|&n| without_prefix.eq_ignore_ascii_case(n))
}

fn strip_opt_val<'a>(arg: &'a str, names: &[&str]) -> Option<&'a str> {
    let s = arg.trim();
    let rest = if let Some(r) = s.strip_prefix("--") {
        r
    } else if let Some(r) = s.strip_prefix('-') {
        r
    } else if let Some(r) = s.strip_prefix('/') {
        r
    } else {
        return None;
    };

    for &name in names {
        if let Some(after) = rest.get(..name.len()) {
            if after.eq_ignore_ascii_case(name) {
                let remainder = &rest[name.len()..];
                if remainder.starts_with('=') || remainder.starts_with(':') {
                    return Some(&remainder[1..]);
                }
            }
        }
    }
    None
}

fn mask_args(args: &[String]) -> Vec<String> {
    let mut masked = Vec::new();
    let mut skip_next = false;
    for arg in args {
        if skip_next {
            masked.push("********".to_string());
            skip_next = false;
        } else if is_opt(arg, &["pass"]) {
            masked.push(arg.clone());
            skip_next = true;
        } else if let Some(_) = strip_opt_val(arg, &["pass"]) {
            let prefix = if arg.starts_with("--") {
                "--pass="
            } else if arg.starts_with('/') {
                "/pass="
            } else {
                "-pass="
            };
            masked.push(format!("{}********", prefix));
        } else {
            masked.push(arg.clone());
        }
    }
    masked
}

fn parse_cli_args(raw_args: &[String]) -> (Vec<String>, Option<String>, Option<String>) {
    let mut clean_args = Vec::new();
    let mut cli_pass: Option<String> = None;
    let mut cli_user: Option<String> = None;
    let mut i = 0;
    while i < raw_args.len() {
        let arg = &raw_args[i];
        if is_opt(arg, &["pass"]) {
            if let Some(next) = raw_args.get(i + 1) {
                let trimmed = next.trim().to_string();
                if !trimmed.is_empty() {
                    cli_pass = Some(trimmed);
                }
                i += 2;
                continue;
            }
        } else if let Some(val) = strip_opt_val(arg, &["pass"]) {
            let trimmed = val.trim().to_string();
            if !trimmed.is_empty() {
                cli_pass = Some(trimmed);
            }
            i += 1;
            continue;
        } else if is_opt(arg, &["user"]) {
            if let Some(next) = raw_args.get(i + 1) {
                let trimmed = next.trim().to_string();
                if !trimmed.is_empty() {
                    cli_user = Some(trimmed);
                }
                i += 2;
                continue;
            }
        } else if let Some(val) = strip_opt_val(arg, &["user"]) {
            let trimmed = val.trim().to_string();
            if !trimmed.is_empty() {
                cli_user = Some(trimmed);
            }
            i += 1;
            continue;
        } else if is_opt(arg, &["debug", "d"]) {
            set_debug_mode(true);
            i += 1;
            continue;
        }
        clean_args.push(arg.clone());
        i += 1;
    }
    (clean_args, cli_pass, cli_user)
}

fn print_help() {
    println!("auximap v0.3.1 - PPx aux: path IMAP bridge");
    println!("A lightweight CLI bridge between Paper Plane xUI (PPx) aux: path and IMAP mail.");
    println!();
    println!("Copyright (c) 2026 auximap contributors");
    println!();
    println!("Usage:");
    println!("  auximap <command> [arguments...] [-debug] [--pass <password>] [--user <user_or_email>]");
    println!();
    println!("Commands:");
    println!("  list   <account/folder> <output_file> [--chunk [N]] [--offset N] [--limit N]");
    println!("         Fetch email headers and generate PPx aux list format.");
    println!("  read   <account/folder> <uid> [dest_file]");
    println!("         Fetch and format email body (text/markdown).");
    println!("  get    <src_file> <dest_file>");
    println!("         Export raw .eml file to destination.");
    println!("  copy   <src_file> <dest_folder>");
    println!("         Copy email between folders/accounts.");
    println!("  move   <src_file> <dest_folder>");
    println!("         Move email between folders/accounts.");
    println!("  delete <src_file> | <account/folder> <uid1> [uid2...]");
    println!("         Delete email(s) or move to Trash.");
    println!("  makedir <account/folder>");
    println!("         Create a mailbox / IMAP folder.");
    println!("  deldir  <account/folder>");
    println!("         Delete a mailbox / IMAP folder.");
    println!("  rename  <src_folder> <dest_folder>");
    println!("         Rename a mailbox / IMAP folder.");
    println!();
    println!("Password Configuration:");
    println!("  Method 1: Direct in auximap.ini (pass = your_password)");
    println!("  Method 2: Via CLI option --pass (e.g. PPx %*pass dialog, highest priority)");
    println!("  Method 3: Environment variable AUXIMAP_PASS_<ACCOUNT> or AUXIMAP_PASS");
    println!();
    println!("User Configuration:");
    println!("  Method 1: Direct in auximap.ini (user = your_email@example.com)");
    println!("  Method 2: Via CLI option --user (e.g. PPx %*user dialog, domain auto-configured)");
    println!();
    println!("Options:");
    println!("  -d, -debug, --debug   Enable debug logging (stderr + auximap.log).");
    println!("  -o, --offset <count>  Skip count of newest messages (e.g. --offset 3000).");
    println!("  -l, --limit <count>   Maximum messages to fetch (e.g. --limit 100).");
    println!("  -c, --chunk <size>    Batch fetch chunk size (default: 250).");
    println!("  --pass <password>     Provide mail password directly (highest priority, e.g. PPx %*pass).");
    println!("  --user <email>        Provide user / email address directly (e.g. PPx %*user).");
    println!("  -h, -help, --help     Show this help message.");
}

fn main() {
    let raw_args: Vec<String> = env::args().collect();
    let (args, cli_pass, cli_user) = parse_cli_args(&raw_args);
    log_msg(&format!("START: {:?}", mask_args(&raw_args)));

    if args.len() < 2 {
        print_help();
        return;
    }

    let command = args[1].to_lowercase();
    if matches!(command.as_str(), "help" | "--help" | "-help" | "-h" | "/h" | "/help" | "/?" | "-?") {
        print_help();
        return;
    }

    let pass_ref = cli_pass.as_deref();
    let user_ref = cli_user.as_deref();

    let result = match command.as_str() {
        "read" | "view" => {
            if args.len() < 3 {
                Err(anyhow!("Usage: auximap read <path> [dest_file]  or  auximap read <folder> <uid_or_file> [dest_file]"))
            } else {
                let (src_mail, dest_opt) = if args.len() >= 4 {
                    let p2 = parse_mail_path(&args[2]);
                    if p2.uid.is_some() {
                        (p2, Some(args[3].as_str()))
                    } else if let Ok(uid) = args[3].trim().parse::<u32>() {
                        let mut p = p2;
                        p.uid = Some(uid);
                        let dest = args.get(4).map(|s| s.as_str());
                        (p, dest)
                    } else if args[3].starts_with('[') || args[3].ends_with(".eml") {
                        let full = format!("{}/{}", args[2].trim_end_matches(['/', '\\']), args[3].trim_start_matches(['/', '\\']));
                        let p = parse_mail_path(&full);
                        let dest = args.get(4).map(|s| s.as_str());
                        (p, dest)
                    } else {
                        (p2, Some(args[3].as_str()))
                    }
                } else {
                    let p = parse_mail_path(&args[2]);
                    (p, None)
                };

                cmd_read(&src_mail, dest_opt, pass_ref, user_ref)
            }
        }
        "list" => {
            let target = args.get(2).map(|s| s.as_str()).unwrap_or("");
            let out = args.get(3).map(|s| s.as_str()).unwrap_or("");
            if out.is_empty() {
                Err(anyhow!("Output file required for list command"))
            } else {
                let mut cli_chunk = None;
                let mut cli_offset = None;
                let mut cli_limit = None;
                let mut i = 4;
                while i < args.len() {
                    let arg = &args[i];
                    if is_opt(arg, &["chunk", "c"]) {
                        if let Some(next_arg) = args.get(i + 1) {
                            if let Ok(size) = next_arg.parse::<u32>() {
                                cli_chunk = Some(size);
                                i += 2;
                                continue;
                            }
                        }
                        cli_chunk = Some(250);
                    } else if let Some(val) = strip_opt_val(arg, &["chunk", "c"]) {
                        if let Ok(size) = val.parse::<u32>() {
                            cli_chunk = Some(size);
                        } else {
                            cli_chunk = Some(250);
                        }
                    } else if is_opt(arg, &["offset", "o"]) {
                        if let Some(next_arg) = args.get(i + 1) {
                            if let Ok(off) = next_arg.parse::<u32>() {
                                cli_offset = Some(off);
                                i += 2;
                                continue;
                            }
                        }
                    } else if let Some(val) = strip_opt_val(arg, &["offset", "o"]) {
                        if let Ok(off) = val.parse::<u32>() {
                            cli_offset = Some(off);
                        }
                    } else if is_opt(arg, &["limit", "l"]) {
                        if let Some(next_arg) = args.get(i + 1) {
                            if let Ok(lim) = next_arg.parse::<u32>() {
                                cli_limit = Some(lim);
                                i += 2;
                                continue;
                            }
                        }
                    } else if let Some(val) = strip_opt_val(arg, &["limit", "l"]) {
                        if let Ok(lim) = val.parse::<u32>() {
                            cli_limit = Some(lim);
                        }
                    }
                    i += 1;
                }
                cmd_list(target, out, cli_chunk, cli_offset, cli_limit, pass_ref, user_ref)
            }
        }
        "get" => {
            if args.len() < 4 {
                Err(anyhow!("Usage: imapmail get <src_file> <dest>"))
            } else {
                let (src_mail, dest) = if args.len() >= 5 {
                    let folder = &args[2];
                    let file = &args[3];
                    let dest = &args[4];
                    let full = format!("{}/{}", folder.trim_end_matches('/'), file);
                    (parse_mail_path(&full), dest.to_string())
                } else {
                    let src = &args[2];
                    let dest = &args[3];
                    (parse_mail_path(src), dest.to_string())
                };
                cmd_get(&src_mail, &dest, pass_ref, user_ref)
            }
        }
        "copy" => {
            if args.len() < 4 {
                Err(anyhow!("Usage: imapmail copy <src_file> <dest>"))
            } else {
                let (src_mail, dest) = if args.len() >= 5 {
                    let folder = &args[2];
                    let file = &args[3];
                    let dest = &args[4];
                    let full = format!("{}/{}", folder.trim_end_matches('/'), file);
                    (parse_mail_path(&full), dest.to_string())
                } else {
                    let src = &args[2];
                    let dest = &args[3];
                    (parse_mail_path(src), dest.to_string())
                };
                cmd_copy(&src_mail, &dest, pass_ref, user_ref)
            }
        }
        "move" => {
            if args.len() < 4 {
                Err(anyhow!("Usage: imapmail move <src_file> <dest>"))
            } else {
                let (src_mail, dest) = if args.len() >= 5 {
                    let folder = &args[2];
                    let file = &args[3];
                    let dest = &args[4];
                    let full = format!("{}/{}", folder.trim_end_matches('/'), file);
                    (parse_mail_path(&full), dest.to_string())
                } else {
                    let src = &args[2];
                    let dest = &args[3];
                    (parse_mail_path(src), dest.to_string())
                };
                cmd_move(&src_mail, &dest, pass_ref, user_ref)
            }
        }
        "delete" => {
            if args.len() < 3 {
                Err(anyhow!("Usage: auximap delete <src_file>  or  auximap delete <account/folder> <uid1> [uid2 ...]"))
            } else if args.len() >= 4 && !args[3].is_empty() {
                let p2 = parse_mail_path(&args[2]);
                let mut uids = Vec::new();
                for arg in &args[3..] {
                    if let Some(uid) = extract_uid(arg) {
                        uids.push(uid);
                    }
                }
                if !uids.is_empty() && p2.account.is_some() && p2.folder.is_some() {
                    cmd_delete_multi(p2.account.as_deref().unwrap(), p2.folder.as_deref().unwrap(), &uids, pass_ref, user_ref)
                } else {
                    let folder = &args[2];
                    let file = &args[3];
                    let full = format!("{}/{}", folder.trim_end_matches('/'), file);
                    let src_mail = parse_mail_path(&full);
                    cmd_delete(&src_mail, pass_ref, user_ref)
                }
            } else {
                let src = &args[2];
                let src_mail = parse_mail_path(src);
                cmd_delete(&src_mail, pass_ref, user_ref)
            }
        }
        "makedir" => {
            let target = if args.len() >= 4 && !args[3].is_empty() {
                format!("{}/{}", args[2].trim_end_matches(['/', '\\']), args[3].trim_start_matches(['/', '\\']))
            } else {
                args.get(2).map(|s| s.as_str()).unwrap_or("").to_string()
            };
            cmd_makedir(&target, pass_ref, user_ref)
        }
        "deldir" => {
            let target = if args.len() >= 4 && !args[3].is_empty() {
                format!("{}/{}", args[2].trim_end_matches(['/', '\\']), args[3].trim_start_matches(['/', '\\']))
            } else {
                args.get(2).map(|s| s.as_str()).unwrap_or("").to_string()
            };
            cmd_deldir(&target, pass_ref, user_ref)
        }
        "rename" => {
            if args.len() < 4 {
                Err(anyhow!("Usage: auximap rename <src_path> <dest_path>"))
            } else {
                let src = &args[2];
                let dest = &args[3];
                cmd_rename(src, dest, pass_ref, user_ref)
            }
        }
        _ => Err(anyhow!("Unknown command: {}", command)),
    };

    if let Err(e) = &result {
        log_msg(&format!("ERROR: {:?}", e));
        std::process::exit(1);
    } else {
        log_msg("SUCCESS");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mail_path() {
        let p1 = parse_mail_path("aux://S_imapmail/sakura/INBOX/[12345] sender - subject.eml");
        assert_eq!(p1.account.as_deref(), Some("sakura"));
        assert_eq!(p1.folder.as_deref(), Some("INBOX"));
        assert_eq!(p1.filename.as_deref(), Some("[12345] sender - subject.eml"));
        assert_eq!(p1.uid, Some(12345));

        let p2 = parse_mail_path("sakura/INBOX.Archive/[999] test.eml");
        assert_eq!(p2.account.as_deref(), Some("sakura"));
        assert_eq!(p2.folder.as_deref(), Some("INBOX.Archive"));
        assert_eq!(p2.filename.as_deref(), Some("[999] test.eml"));
        assert_eq!(p2.uid, Some(999));

        let p3 = parse_mail_path("aux://S_imapmail/sakura/INBOX");
        assert_eq!(p3.account.as_deref(), Some("sakura"));
        assert_eq!(p3.folder.as_deref(), Some("INBOX"));
        assert_eq!(p3.filename, None);

        let p4 = parse_mail_path("sakura");
        assert_eq!(p4.account.as_deref(), Some("sakura"));
        assert_eq!(p4.folder, None);
        assert_eq!(p4.filename, None);

        let p5 = parse_mail_path("sakura/INBOX/SubFolder/[555] hello.eml");
        assert_eq!(p5.account.as_deref(), Some("sakura"));
        assert_eq!(p5.folder.as_deref(), Some("INBOX/SubFolder"));
        assert_eq!(p5.uid, Some(555));

        // Windows バックスラッシュ & aux: バリエーションのテスト
        let p6 = parse_mail_path(r"aux://S_imapmail\sakura\INBOX");
        assert_eq!(p6.account.as_deref(), Some("sakura"));
        assert_eq!(p6.folder.as_deref(), Some("INBOX"));

        let p7 = parse_mail_path(r"aux:S_imapmail\sakura\仕事");
        assert_eq!(p7.account.as_deref(), Some("sakura"));
        assert_eq!(p7.folder.as_deref(), Some("仕事"));

        let p8 = parse_mail_path(r#""aux://S_imapmail/sakura/仕事""#);
        assert_eq!(p8.account.as_deref(), Some("sakura"));
        assert_eq!(p8.folder.as_deref(), Some("仕事"));

        let p9 = parse_mail_path("aux://S_auximap/sakura/INBOX/[12345] test.eml");
        assert_eq!(p9.account.as_deref(), Some("sakura"));
        assert_eq!(p9.folder.as_deref(), Some("INBOX"));
        assert_eq!(p9.uid, Some(12345));
    }

    #[test]
    fn test_is_local_path() {
        assert!(is_local_path(r"C:\Users\username\Desktop"));
        assert!(is_local_path("D:/Mails/test.eml"));
        assert!(is_local_path(r"\\server\share"));
        assert!(!is_local_path("aux://S_imapmail/sakura/INBOX"));
        assert!(!is_local_path("sakura/Trash"));
    }

    #[test]
    fn test_modified_utf7_roundtrip() {
        let cases = [
            ("INBOX.Archive", "INBOX.Archive"),
            ("INBOX.迷惑メール", "INBOX.&j,dg0TDhMPww6w-"),
            ("INBOX.証券会社", "INBOX.&ijxSOE8aeT4-"),
            ("INBOX.白饅頭日誌", "INBOX.&dn2ZRZgtZeWKjA-"),
            ("INBOX.未整理1", "INBOX.&ZypldHQG-1"),
            ("INBOX.会員登録", "INBOX.&TxpU4XZ7kzI-"),
            ("INBOX.会社から", "INBOX.&Txp5PjBLMIk-"),
            ("INBOX.下書き", "INBOX.&Tgtm+DBN-"),
            ("INBOX.ショッピング", "INBOX.&MLcw5zDDMNQw8zCw-"),
            ("INBOX.オークション", "INBOX.&MKow,DCvMLcw5zDz-"),
            ("INBOX.クレジットカード", "INBOX.&MK8w7DC4MMMwyDCrMPwwyQ-"),
            ("INBOX.その他", "INBOX.&MF0wbk7W-"),
            ("Test &- Co", "Test &-- Co"),
            ("~peter/mail/&/台北/日本語", "~peter/mail/&-/&U,BTFw-/&ZeVnLIqe-"),
        ];

        for (plain, encoded) in cases {
            assert_eq!(encode_modified_utf7(plain), encoded, "Encoding mismatch for: {}", plain);
            assert_eq!(decode_modified_utf7(encoded), plain, "Decoding mismatch for: {}", encoded);
        }
    }

    #[test]
    fn test_truncate_str() {
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(truncate_str("1234567890", 5), "12...");
        assert_eq!(truncate_str("あいうえおかきくけこ", 6), "あいう...");
    }

    #[test]
    fn test_underscore_mime_decode() {
        let raw = "=_utf-8_q_=E3=83=A1=E3=83=AB=E3=82=AB=E3=83=AA_=";
        assert_eq!(scan_and_decode_rfc2047(raw), "メルカリ");

        let raw_standard = "=?UTF-8?B?44Oh44Or44Kr44Oq?=";
        assert_eq!(scan_and_decode_rfc2047(raw_standard), "メルカリ");
    }

    #[test]
    fn test_strip_html_tags() {
        let html = "<html><head><style>body { color: red; }</style></head><body><p>こんにちは</p><br><div>テスト&amp;メール &lt;OK&gt;</div><script>console.log('hi');</script></body></html>";
        let text = strip_html_tags(html);
        assert!(text.contains("こんにちは"));
        assert!(text.contains("テスト&メール <OK>"));
        assert!(!text.contains("body { color: red; }"));
        assert!(!text.contains("console.log"));
    }

    #[test]
    fn test_apply_template() {
        let tmpl = "From: {from}\nTo: {to}\nCc: {cc}\nSubject: {subject}\nAttach: {attachments}\n---\n{body}";

        // Cc と Attachments が空の場合、その行が省略されるか
        let res = apply_template(
            tmpl,
            "sender@example.com",
            "to@example.com",
            "",
            "2026-09-18 01:00:00",
            "テスト件名",
            "",
            "テスト本文です。",
        );

        assert!(res.contains("From: sender@example.com"));
        assert!(res.contains("To: to@example.com"));
        assert!(!res.contains("Cc:"));
        assert!(!res.contains("Attach:"));
        assert!(res.contains("テスト件名"));
        assert!(res.contains("テスト本文です。"));
    }

    #[test]
    fn test_format_mail_content() {
        let raw_eml = b"From: =?UTF-8?B?5bGx55Sw?= <yamada@example.com>\r\n\
To: target@example.com\r\n\
Subject: =?UTF-8?B?44OG44K544OI44Oh44O844Or?=\r\n\
Date: Fri, 18 Sep 2026 02:00:00 +0900\r\n\
Content-Type: text/plain; charset=UTF-8\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
44GT44KT44Gr44Gh44Gv44CB44OG44K544OI44Gn44GZ44CC\r\n";

        let view_conf = ViewConfig {
            template_file: None,
            template_inline: None,
            html_policy: "text".to_string(),
        };

        let formatted = format_mail_content(raw_eml, "local", &view_conf, Path::new(".")).unwrap();
        assert!(formatted.contains("山田"));
        assert!(formatted.contains("yamada@example.com"));
        assert!(formatted.contains("テストメール"));
        assert!(formatted.contains("こんにちは、テストです。"));
    }

    #[test]
    fn test_parse_cli_args() {
        // --pass value と --user value の抽出
        let raw1 = vec![
            "auximap".to_string(),
            "list".to_string(),
            "sakura/INBOX".to_string(),
            "out.txt".to_string(),
            "--pass".to_string(),
            "secret123".to_string(),
            "--user".to_string(),
            "user1@example.com".to_string(),
            "--chunk".to_string(),
        ];
        let (clean1, pass1, user1) = parse_cli_args(&raw1);
        assert_eq!(pass1.as_deref(), Some("secret123"));
        assert_eq!(user1.as_deref(), Some("user1@example.com"));
        assert_eq!(clean1, vec!["auximap", "list", "sakura/INBOX", "out.txt", "--chunk"]);

        // --pass=value と --user=value の抽出
        let raw2 = vec![
            "auximap".to_string(),
            "read".to_string(),
            "sakura/INBOX".to_string(),
            "100".to_string(),
            "--pass=secret456".to_string(),
            "--user=user2@example.com".to_string(),
        ];
        let (clean2, pass2, user2) = parse_cli_args(&raw2);
        assert_eq!(pass2.as_deref(), Some("secret456"));
        assert_eq!(user2.as_deref(), Some("user2@example.com"));
        assert_eq!(clean2, vec!["auximap", "read", "sakura/INBOX", "100"]);

        // -pass value と -user value と -debug の抽出 (ハイフン1つ・スラッシュ)
        let raw4 = vec![
            "auximap".to_string(),
            "-debug".to_string(),
            "list".to_string(),
            "-pass".to_string(),
            "secret789".to_string(),
            "/user:user3@example.com".to_string(),
        ];
        let (clean4, pass4, user4) = parse_cli_args(&raw4);
        assert_eq!(pass4.as_deref(), Some("secret789"));
        assert_eq!(user4.as_deref(), Some("user3@example.com"));
        assert_eq!(clean4, vec!["auximap", "list"]);
    }

    #[test]
    fn test_domain_rule_matching() {
        assert!(match_rule_pattern("gmail.com", "gmail.com"));
        assert!(match_rule_pattern("gmail.com", "GMAIL.COM"));
        assert!(!match_rule_pattern("gmail.com", "yahoo.co.jp"));

        // ワイルドカード *.sakura.ne.jp
        assert!(match_rule_pattern("*.sakura.ne.jp", "example.sakura.ne.jp"));
        assert!(match_rule_pattern("*.sakura.ne.jp", "sub.tenant.sakura.ne.jp"));
        assert!(match_rule_pattern("*.sakura.ne.jp", "sakura.ne.jp"));
        assert!(!match_rule_pattern("*.sakura.ne.jp", "example.com"));

        // DEFAULT / *
        assert!(match_rule_pattern("default", "anydomain.com"));
        assert!(match_rule_pattern("*", "anydomain.com"));
    }

    #[test]
    fn test_expand_template() {
        let tmpl1 = "{domain}";
        assert_eq!(expand_template(tmpl1, "info@example.sakura.ne.jp", "example.sakura.ne.jp"), "example.sakura.ne.jp");

        let tmpl2 = "mail.{domain}";
        assert_eq!(expand_template(tmpl2, "user@domain.com", "domain.com"), "mail.domain.com");

        let tmpl3 = "imap.{user_part}.{domain}";
        assert_eq!(expand_template(tmpl3, "admin@host.net", "host.net"), "imap.admin.host.net");
    }

    #[test]
    fn test_calculate_seq_range() {
        // 通常の取得（offset = 0）
        assert_eq!(calculate_seq_range(10000, 100, 0), Some((9901, 10000)));

        // 3000件スキップして100件取得
        assert_eq!(calculate_seq_range(10000, 100, 3000), Some((6901, 7000)));

        // 残り件数が limit 未満の場合（最古の seq 1 まで）
        assert_eq!(calculate_seq_range(10000, 100, 9950), Some((1, 50)));

        // offset が総件数ちょうど、または超過した場合は取得対象なし
        assert_eq!(calculate_seq_range(10000, 100, 10000), None);
        assert_eq!(calculate_seq_range(10000, 100, 12000), None);

        // 総件数が 0 の場合
        assert_eq!(calculate_seq_range(0, 100, 0), None);

        // 総件数が limit 未満（例: 50件中 offset 0, limit 100）
        assert_eq!(calculate_seq_range(50, 100, 0), Some((1, 50)));

        // 総件数が limit 未満で offset あり（例: 50件中 offset 20, limit 100）
        assert_eq!(calculate_seq_range(50, 100, 20), Some((1, 30)));
    }

    #[test]
    fn test_mask_args() {
        let args1 = vec!["auximap".into(), "list".into(), "--pass".into(), "secret123".into(), "--user".into(), "test@example.com".into()];
        let masked1 = mask_args(&args1);
        assert_eq!(masked1, vec!["auximap", "list", "--pass", "********", "--user", "test@example.com"]);

        let args2 = vec!["auximap".into(), "copy".into(), "--pass=secret456".into(), "dest".into()];
        let masked2 = mask_args(&args2);
        assert_eq!(masked2, vec!["auximap", "copy", "--pass=********", "dest"]);

        let args3 = vec!["auximap".into(), "-pass".into(), "secret789".into(), "/pass:secret000".into()];
        let masked3 = mask_args(&args3);
        assert_eq!(masked3, vec!["auximap", "-pass", "********", "/pass=********"]);
    }
}

