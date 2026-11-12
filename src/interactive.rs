//! Slash commands that ask with pickers: model, provider, login, logout,
//! effort and resume.

use crate::agent::Agent;
use crate::menu::{self, Choice, Picker};
use crate::provider::{self, Model};
use crate::{auth, session, ui};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

/// Model lists fetched this session, by provider.
pub type ModelCache = HashMap<String, Vec<String>>;

fn display_name(provider: &str) -> String {
    match provider {
        "anthropic" => "Anthropic".into(),
        "openai" => "OpenAI".into(),
        "chatgpt" => "ChatGPT subscription".into(),
        "openrouter" => "OpenRouter".into(),
        other => format!("{other} (OpenAI-compatible)"),
    }
}

fn note(term: &ui::Term, s: &str) {
    eprintln!("{}", term.dim(s));
}

/// A provider's models as the browser shows them, with the full
/// `provider/model` name of each.
struct ModelGroup {
    name: String,
    signed_in: bool,
    choices: Vec<Choice>,
    specs: Vec<String>,
}

/// `262144` as `262k`, `1000000` as `1M`.
fn short_tokens(n: u64) -> String {
    if n >= 1_000_000 && n.is_multiple_of(1_000_000) {
        format!("{}M", n / 1_000_000)
    } else if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else {
        format!("{}k", n / 1000)
    }
}

fn model_detail(spec: &str, provider: &str, current: &str, default: Option<&str>) -> String {
    let info = provider::info(spec);
    let model = Model::parse(spec);
    // Size and marks first: a long description is what gets cut short.
    let mut parts = Vec::new();
    // Context sizes axio knows rather than guesses: listed, or Claude's.
    let window = info
        .as_ref()
        .and_then(|i| i.window)
        .or((model.api == provider::Api::Anthropic).then(|| model.context_window()));
    if let Some(w) = window {
        parts.push(short_tokens(w));
    }
    if Some(spec) == default {
        parts.push("default".into());
    }
    if spec == current {
        parts.push("in use".into());
    }
    parts.push(
        info.and_then(|i| i.about)
            .unwrap_or_else(|| display_name(provider)),
    );
    parts.join(" · ")
}

/// The browser's groups: All, each signed-in provider, then providers that
/// are not set up. Returns them with the group and choice to open on.
fn model_groups(
    lists: &[(String, Vec<String>)],
    not_set_up: &[String],
    current: &str,
    default: Option<&str>,
) -> (Vec<ModelGroup>, usize, Option<usize>) {
    let current_provider = current.split('/').next().unwrap_or("");
    let mut groups = vec![ModelGroup {
        name: "all".into(),
        signed_in: true,
        choices: Vec::new(),
        specs: Vec::new(),
    }];
    for (p, ids) in lists {
        let mut specs = ids.clone();
        // A model in use that its list lacks stays first, so Enter keeps it.
        if p == current_provider && !specs.iter().any(|s| s == current) {
            specs.insert(0, current.to_string());
        }
        let choices = specs
            .iter()
            .map(|spec| {
                let short = spec.strip_prefix(&format!("{p}/")).unwrap_or(spec);
                let detail = if !ids.contains(spec) {
                    "in use".to_string()
                } else {
                    model_detail(spec, p, current, default)
                };
                Choice::new(short, detail)
            })
            .collect::<Vec<_>>();
        groups[0].choices.extend(
            specs
                .iter()
                .zip(&choices)
                .map(|(spec, c)| Choice::new(spec, c.detail.clone())),
        );
        groups[0].specs.extend(specs.iter().cloned());
        groups.push(ModelGroup {
            name: p.clone(),
            signed_in: true,
            choices,
            specs,
        });
    }
    for p in not_set_up.iter().cloned().chain(["other".to_string()]) {
        groups.push(ModelGroup {
            name: p,
            signed_in: false,
            choices: Vec::new(),
            specs: Vec::new(),
        });
    }
    let at = groups
        .iter()
        .position(|g| g.name == current_provider)
        .unwrap_or(0);
    let chosen = groups[at].specs.iter().position(|s| s == current);
    (groups, at, chosen)
}

/// Fetches the model lists of `providers` at once, reusing earlier lists.
fn load_models(providers: &[String], cache: &mut ModelCache, term: &ui::Term) {
    let missing: Vec<String> = providers
        .iter()
        .filter(|p| !cache.contains_key(*p))
        .cloned()
        .collect();
    if missing.is_empty() {
        return;
    }
    eprint!(
        "{}",
        term.dim(&format!("listing models from {}…", missing.join(", ")))
    );
    let _ = std::io::stderr().flush();
    let handles: Vec<_> = missing
        .into_iter()
        .map(|p| std::thread::spawn(move || (p.clone(), provider::models(&p))))
        .collect();
    let mut failed = Vec::new();
    for h in handles {
        if let Ok((p, result)) = h.join() {
            match result {
                Ok(ids) => {
                    cache.insert(p, ids);
                }
                Err(e) => failed.push(format!("{p}: could not list models: {e}")),
            }
        }
    }
    eprint!("\r\x1b[K");
    for f in failed {
        note(term, &f);
    }
}

/// `/model` with no argument: every signed-in provider's models.
pub fn pick_model(
    agent: &mut Agent,
    term: &mut ui::Term,
    cache: &mut ModelCache,
    only: Option<&str>,
) {
    let home = crate::context::home();
    let mut open_on = only.map(String::from);
    loop {
        let mut providers = provider::signed_in();
        if !providers.contains(&agent.model.provider) {
            providers.push(agent.model.provider.clone());
        }
        load_models(&providers, cache, term);
        let lists: Vec<(String, Vec<String>)> = providers
            .iter()
            .map(|p| (p.clone(), cache.get(p).cloned().unwrap_or_default()))
            .collect();
        let not_set_up: Vec<String> = provider::KNOWN
            .iter()
            .filter(|p| !providers.iter().any(|q| q == *p))
            .map(|p| p.to_string())
            .collect();
        let default = crate::settings::Settings::load(&home, None)
            .ok()
            .and_then(|s| s.model);
        let current = agent.model.to_string();
        let (groups, mut at, mut chosen) =
            model_groups(&lists, &not_set_up, &current, default.as_deref());
        if let Some(p) = open_on.take()
            && let Some(i) = groups.iter().position(|g| g.name == p)
            && i != at
        {
            (at, chosen) = (i, None);
        }
        let effort = agent
            .effort
            .clone()
            .or_else(|| agent.model.default_effort());
        let specs: Vec<Vec<String>> = groups.iter().map(|g| g.specs.clone()).collect();
        let names: Vec<String> = groups.iter().map(|g| g.name.clone()).collect();
        let menu_groups = groups
            .into_iter()
            .map(|g| {
                let levels = g.specs.iter().map(|s| Model::parse(s).efforts()).collect();
                // A bare name typed in stays with this provider, or the one in use.
                let prefix = match g.name.as_str() {
                    "all" => format!("{}/", agent.model.provider),
                    p => format!("{p}/"),
                };
                menu::Group {
                    name: g.name,
                    signed_in: g.signed_in,
                    picker: Picker::new("", g.choices, None)
                        .allow_typed(&prefix)
                        .with_effort(levels, effort.as_deref()),
                }
            })
            .collect();
        let mut browser = menu::Browser::new(menu_groups, at, chosen);
        let keys = term.keys.as_mut().expect("keys");
        let (spec, scope) = match menu::browse(keys, &mut browser, term.color) {
            menu::Browse::Picked(g, i, scope) => (specs[g][i].clone(), scope),
            // A model the list did not show, such as on a server with no list.
            menu::Browse::Typed(_, id, scope) => (id, scope),
            menu::Browse::SignIn(g) => {
                if let Some(name) = login_to(term, &names[g]) {
                    cache.remove(&name);
                    open_on = Some(name);
                    continue;
                }
                return;
            }
            _ => return note(term, &format!("model: {} (unchanged)", agent.model)),
        };
        agent.model = Model::parse(&spec);
        if let Some(e) = browser.picker().effort() {
            agent.effort = Some(e);
        }
        let saved = match scope {
            menu::Scope::Default => save_default(&home, &agent.model, agent.effort.as_deref()),
            menu::Scope::Session => Ok("this session"),
        };
        return report_choice(term, agent, saved);
    }
}

/// Saves the model and effort for new sessions.
fn save_default(home: &Path, model: &Model, effort: Option<&str>) -> Result<&'static str, String> {
    crate::settings::set(home, "model", serde_json::json!(model.to_string()))?;
    if let Some(e) = effort {
        crate::settings::set(home, "effort", serde_json::json!(e))?;
    }
    Ok("default for new sessions")
}

fn report_choice(term: &ui::Term, agent: &Agent, saved: Result<&str, String>) {
    let effort = match agent.model.efforts().is_empty() {
        true => " · no effort setting".to_string(),
        false => agent
            .effort
            .as_deref()
            .and_then(|e| menu::clamp(e, &agent.model.efforts()))
            .map_or(String::new(), |e| format!(" · {e} effort")),
    };
    match saved {
        Ok(scope) => note(term, &format!("model: {}{effort} ({scope})", agent.model)),
        Err(e) => note(
            term,
            &format!(
                "model: {}{effort} for this session; could not save the default: {e}",
                agent.model
            ),
        ),
    }
}

fn provider_choices(signed_in: &[String], current: &str) -> Vec<Choice> {
    let mut names: Vec<String> = provider::KNOWN.iter().map(|p| p.to_string()).collect();
    for p in signed_in {
        if !names.contains(p) {
            names.push(p.clone());
        }
    }
    let mut out: Vec<Choice> = names
        .iter()
        .map(|p| {
            let mut detail = if signed_in.contains(p) {
                "signed in".to_string()
            } else {
                "not set up".to_string()
            };
            if p == current {
                detail.push_str(" · in use");
            }
            Choice::new(p, format!("{} · {detail}", display_name(p)))
        })
        .collect();
    out.push(Choice::new(
        "other",
        "another OpenAI-compatible endpoint, such as a local server",
    ));
    out
}

/// `/provider`: choose one, signing in when it is not set up, then its model.
pub fn pick_provider(agent: &mut Agent, term: &mut ui::Term, cache: &mut ModelCache) {
    let signed_in = provider::signed_in();
    let choices = provider_choices(&signed_in, &agent.model.provider);
    let current = choices.iter().position(|c| c.label == agent.model.provider);
    let labels: Vec<String> = choices.iter().map(|c| c.label.clone()).collect();
    let keys = term.keys.as_mut().expect("keys");
    let Some(i) = menu::pick(keys, Picker::new("Provider", choices, current), term.color) else {
        return;
    };
    let chosen = labels[i].clone();
    let name = if chosen == "other" || !signed_in.contains(&chosen) {
        match login_to(term, &chosen) {
            Some(name) => name,
            None => return,
        }
    } else {
        chosen
    };
    cache.remove(&name);
    pick_model(agent, term, cache, Some(&name));
}

/// Asks for what `provider` needs and saves it; `other` also asks for a name
/// and a base URL. Returns the provider's name once saved.
fn login_to(term: &mut ui::Term, provider: &str) -> Option<String> {
    if provider == crate::chatgpt::PROVIDER {
        return chatgpt_sign_in(term).then(|| provider.to_string());
    }
    let path = auth::path()?.clone();
    let keys = term.keys.as_mut().expect("keys");
    let name = if provider == "other" {
        let name =
            menu::ask(keys, "Name for this provider (letters, digits, -): ", false)?.to_lowercase();
        let reserved = name == "mcp" || provider::KNOWN.contains(&name.as_str());
        if reserved || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            note(term, &format!("{name:?} cannot be used as a provider name"));
            return None;
        }
        let keys = term.keys.as_mut().expect("keys");
        let Some(url) = menu::ask(
            keys,
            "Base URL (such as http://127.0.0.1:11434/v1): ",
            false,
        ) else {
            note(term, "nothing saved");
            return None;
        };
        if let Err(e) = auth::save(&path, &name, "baseUrl", &url) {
            note(term, &format!("axio: {e}"));
            return None;
        }
        name
    } else {
        provider.to_string()
    };
    let optional = provider == "other";
    let question = format!(
        "API key for {}{}: ",
        display_name(&name),
        if optional { " (Enter for none)" } else { "" }
    );
    let keys = term.keys.as_mut().expect("keys");
    match menu::ask(keys, &question, true) {
        Some(key) => {
            if let Err(e) = auth::save(&path, &name, "apiKey", &key) {
                note(term, &format!("axio: {e}"));
                return None;
            }
        }
        None if optional => {}
        None => {
            note(term, "nothing saved");
            return None;
        }
    }
    note(term, &format!("saved {name} to {}", path.display()));
    Some(name)
}

/// Signs in with ChatGPT in the browser. The browser comes back to axio on
/// its own; when it runs elsewhere, the address it ended on can be pasted.
fn chatgpt_sign_in(term: &mut ui::Term) -> bool {
    use crate::term::Key;
    let pending = match crate::chatgpt::start() {
        Ok(p) => p,
        Err(e) => {
            note(term, &format!("axio: {e}"));
            return false;
        }
    };
    eprintln!(
        "Opening your browser to sign in with ChatGPT. If it does not open, visit:\n{}",
        pending.url
    );
    crate::oauth::open_url(&pending.url);
    note(
        term,
        "Waiting for the browser. If it is on another machine, paste the address it ends on and press Enter. Esc cancels.",
    );
    let started = std::time::Instant::now();
    let mut typed = String::new();
    let show = |typed: &str| {
        let room = crate::term::width().saturating_sub(4);
        let tail: String = {
            let chars: Vec<char> = typed.chars().collect();
            chars[chars.len().saturating_sub(room)..].iter().collect()
        };
        print!("\r\x1b[K› {tail}");
        let _ = std::io::stdout().flush();
    };
    show("");
    let reply = loop {
        match pending.poll() {
            Ok(Some(r)) => break Ok(r),
            Err(e) => break Err(e),
            Ok(None) => {}
        }
        if started.elapsed() > std::time::Duration::from_secs(600) {
            break Err("timed out waiting for the browser".into());
        }
        let keys = term.keys.as_mut().expect("keys");
        match keys.next_within(std::time::Duration::from_millis(100)) {
            Some(Key::Esc | Key::Ctrl('c')) => break Err("sign-in cancelled".into()),
            Some(Key::Char(c)) => typed.push(c),
            Some(Key::Paste(t)) => typed.push_str(t.trim()),
            Some(Key::Backspace) => {
                typed.pop();
            }
            Some(Key::Enter) if !typed.trim().is_empty() => match pending.pasted(&typed) {
                Ok(r) => break Ok(r),
                Err(e) => {
                    print!("\r\x1b[K");
                    note(term, &e);
                    typed.clear();
                }
            },
            _ => continue,
        }
        show(&typed);
    };
    print!("\r\x1b[K");
    let _ = std::io::stdout().flush();
    let reply = match reply {
        Ok(r) => r,
        Err(e) => {
            note(term, &format!("axio: {e}"));
            return false;
        }
    };
    if reply.code.is_ok() {
        note(term, "finishing sign-in…");
    }
    let outcome = match &reply.code {
        Ok((code, client)) => pending.finish(code, client),
        Err(e) => Err(e.clone()),
    };
    reply.answer(&outcome);
    match outcome {
        Ok(email) => {
            let who = email.map_or(String::new(), |e| format!(" as {e}"));
            note(term, &format!("signed in with ChatGPT{who}"));
            note(
                term,
                "limits for axio's use of your plan: https://chatgpt.com/settings/usage",
            );
            true
        }
        Err(e) => {
            note(term, &format!("axio: {e}"));
            false
        }
    }
}

/// `/login [provider]`.
pub fn login(agent: &mut Agent, term: &mut ui::Term, cache: &mut ModelCache, arg: &str) {
    let chosen = if arg.is_empty() {
        let signed_in = provider::signed_in();
        let choices = provider_choices(&signed_in, "");
        let labels: Vec<String> = choices.iter().map(|c| c.label.clone()).collect();
        let keys = term.keys.as_mut().expect("keys");
        match menu::pick(keys, Picker::new("Sign in to", choices, None), term.color) {
            Some(i) => labels[i].clone(),
            None => return,
        }
    } else {
        arg.to_string()
    };
    let Some(name) = login_to(term, &chosen) else {
        return;
    };
    // Listing the models checks the key and offers one to switch to.
    cache.remove(&name);
    pick_model(agent, term, cache, Some(&name));
}

/// `/logout [provider]`.
pub fn logout(term: &mut ui::Term, cache: &mut ModelCache, arg: &str) {
    let Some(path) = auth::path().cloned() else {
        return;
    };
    let saved = auth::saved(&path);
    let name = if arg.is_empty() {
        if saved.is_empty() {
            return note(term, "no provider keys are saved");
        }
        let choices = saved
            .iter()
            .map(|p| Choice::new(p, display_name(p)))
            .collect();
        let keys = term.keys.as_mut().expect("keys");
        match menu::pick(
            keys,
            Picker::new("Remove the saved key for", choices, None),
            term.color,
        ) {
            Some(i) => saved[i].clone(),
            None => return,
        }
    } else {
        arg.to_string()
    };
    let forgot = if name == crate::chatgpt::PROVIDER {
        // The issued client stays, so signing in again asks only for the account.
        crate::chatgpt::sign_out()
    } else {
        auth::forget(&path, &name)
    };
    match forgot {
        Ok(true) => {
            cache.remove(&name);
            note(term, &format!("removed {name} from {}", path.display()));
            let env = format!("{}_API_KEY", name.to_uppercase().replace('-', "_"));
            if provider::env(&env).is_some() {
                note(term, &format!("{env} is still set in the environment"));
            }
        }
        Ok(false) => note(term, &format!("{name} has no saved key")),
        Err(e) => note(term, &format!("axio: {e}")),
    }
}

/// `/effort` with no argument: a slider of the levels the model takes.
pub fn pick_effort(agent: &mut Agent, term: &mut ui::Term) {
    let levels = agent.model.efforts();
    if levels.is_empty() {
        return note(term, &format!("{} has no effort setting", agent.model.id));
    }
    let current = agent
        .effort
        .clone()
        .or_else(|| agent.model.default_effort())
        .unwrap_or_else(|| "high".into());
    let title = format!("Effort · {}", agent.model.id);
    let slider = menu::Slider::new(&title, levels.clone(), agent.model.effort_notes(), &current);
    let keys = term.keys.as_mut().expect("keys");
    let Some((i, scope)) = menu::slide(keys, slider, term.color) else {
        return;
    };
    agent.effort = Some(levels[i].clone());
    let saved = match scope {
        menu::Scope::Default => crate::settings::set(
            &crate::context::home(),
            "effort",
            serde_json::json!(levels[i]),
        )
        .map(|()| "default for new sessions"),
        menu::Scope::Session => Ok("this session"),
    };
    match saved {
        Ok(scope) => note(term, &format!("effort: {} ({scope})", levels[i])),
        Err(e) => note(
            term,
            &format!(
                "effort: {} for this session; could not save the default: {e}",
                levels[i]
            ),
        ),
    }
}

pub fn age(secs: u64) -> String {
    match secs {
        0..60 => "just now".to_string(),
        60..3600 => format!("{}m ago", secs / 60),
        3600..86_400 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

/// `/resume`: continue a saved session from this directory.
pub fn resume(agent: &mut Agent, term: &mut ui::Term, dir: &Path) {
    let recent = session::recent(dir, 50);
    let now = std::time::SystemTime::now();
    let current = agent.session.as_ref().map(|s| s.path.clone());
    let recent: Vec<_> = recent
        .into_iter()
        .filter(|s| Some(&s.path) != current.as_ref())
        .collect();
    if recent.is_empty() {
        return note(term, "no other saved sessions here");
    }
    let choices = recent
        .iter()
        .map(|s| {
            let secs = now.duration_since(s.modified).map_or(0, |d| d.as_secs());
            Choice::new(
                s.first_prompt.lines().next().unwrap_or("").to_string(),
                format!("{} · {} messages", age(secs), s.messages),
            )
        })
        .collect();
    let keys = term.keys.as_mut().expect("keys");
    let Some(i) = menu::pick(keys, Picker::new("Resume", choices, Some(0)), term.color) else {
        return;
    };
    let path = recent[i].path.clone();
    let loaded = session::load(&path).and_then(|m| Ok((m, session::Session::open(path)?)));
    match loaded {
        Ok((messages, s)) => {
            agent.messages = messages;
            agent.session = Some(s);
            agent.context = agent.estimated_context();
            note(term, &format!("resumed {} messages", agent.messages.len()));
            for line in ui::recap(&agent.messages) {
                note(term, &line);
            }
        }
        Err(e) => note(term, &format!("axio: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_list_every_provider_with_the_current_model_selected() {
        let lists = vec![
            (
                "anthropic".to_string(),
                vec![
                    "anthropic/claude-haiku-4-5".to_string(),
                    "anthropic/claude-opus-5-5".to_string(),
                ],
            ),
            ("local".to_string(), vec!["local/qwen".to_string()]),
        ];
        provider::learn(
            "local/qwen",
            provider::Info {
                about: Some("Qwen here".into()),
                window: Some(262_144),
                ..provider::Info::default()
            },
        );
        let (groups, at, current) = model_groups(
            &lists,
            &["openai".to_string()],
            "anthropic/claude-opus-5-5",
            Some("local/qwen"),
        );
        let names: Vec<(&str, bool)> = groups
            .iter()
            .map(|g| (g.name.as_str(), g.signed_in))
            .collect();
        assert_eq!(
            names,
            [
                ("all", true),
                ("anthropic", true),
                ("local", true),
                ("openai", false),
                ("other", false)
            ]
        );
        assert_eq!(
            (at, current),
            (1, Some(1)),
            "opens on the provider and model in use"
        );
        assert_eq!(groups[0].specs.len(), 3);
        assert_eq!(
            groups[0].choices[2].label, "local/qwen",
            "All shows full names"
        );
        assert_eq!(
            groups[2].choices[0].label, "qwen",
            "a provider shows short names"
        );
        assert_eq!(groups[2].choices[0].detail, "262k · default · Qwen here");
        assert_eq!(groups[2].specs[0], "local/qwen");
        assert_eq!(groups[1].choices[1].detail, "1M · in use · Anthropic");
        // A model in use that no list holds is kept at the top of its provider.
        let (groups, at, current) = model_groups(&lists, &[], "local/llama", None);
        assert_eq!((at, current), (2, Some(0)));
        assert_eq!(groups[2].specs[0], "local/llama");
        assert_eq!(groups[2].choices[0].detail, "in use");
    }

    #[test]
    fn provider_choices_show_what_is_set_up() {
        let c = provider_choices(&["anthropic".into(), "local".into()], "anthropic");
        let rows: Vec<(&str, &str)> = c
            .iter()
            .map(|c| (c.label.as_str(), c.detail.as_str()))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("anthropic", "Anthropic · signed in · in use"),
                ("chatgpt", "ChatGPT subscription · not set up"),
                ("openai", "OpenAI · not set up"),
                ("openrouter", "OpenRouter · not set up"),
                ("local", "local (OpenAI-compatible) · signed in"),
                (
                    "other",
                    "another OpenAI-compatible endpoint, such as a local server"
                ),
            ]
        );
    }
}
