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
        "openrouter" => "OpenRouter".into(),
        other => format!("{other} (OpenAI-compatible)"),
    }
}

fn note(term: &ui::Term, s: &str) {
    eprintln!("{}", term.dim(s));
}

/// One choice per listed model, the one in use first selected. A model in
/// use that no list holds is kept at the top so Enter leaves it alone.
fn model_choices(lists: &[(String, Vec<String>)], current: &str) -> (Vec<Choice>, usize) {
    let mut choices: Vec<Choice> = lists
        .iter()
        .flat_map(|(p, ids)| ids.iter().map(move |id| Choice::new(id, display_name(p))))
        .collect();
    let at = match choices.iter().position(|c| c.label == current) {
        Some(i) => {
            choices[i].detail.push_str(" · in use");
            i
        }
        None => {
            choices.insert(0, Choice::new(current, "in use"));
            0
        }
    };
    (choices, at)
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
    let mut providers = match only {
        Some(p) => vec![p.to_string()],
        None => provider::signed_in(),
    };
    if only.is_none() && !providers.contains(&agent.model.provider) {
        providers.push(agent.model.provider.clone());
    }
    load_models(&providers, cache, term);
    let lists: Vec<(String, Vec<String>)> = providers
        .iter()
        .filter_map(|p| cache.get(p).map(|ids| (p.clone(), ids.clone())))
        .collect();
    let current = agent.model.to_string();
    let (choices, at) = model_choices(&lists, &current);
    let title = match only {
        Some(p) => format!("Model from {}", display_name(p)),
        None => "Model".into(),
    };
    let labels: Vec<String> = choices.iter().map(|c| c.label.clone()).collect();
    let keys = term.keys.as_mut().expect("keys");
    // A bare name typed in stays with the provider being shown, or in use.
    let prefix = format!("{}/", only.unwrap_or(&agent.model.provider));
    let picker = Picker::new(&title, choices, Some(at)).allow_typed(&prefix);
    match menu::choose(keys, picker, term.color) {
        menu::Step::Picked(i) => agent.model = Model::parse(&labels[i]),
        // A model the list did not show, such as on a server with no list.
        menu::Step::Typed(id) => agent.model = Model::parse(&id),
        _ => {}
    }
    note(term, &format!("model: {}", agent.model));
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
    match auth::forget(&path, &name) {
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

const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// `/effort` with no argument.
pub fn pick_effort(agent: &mut Agent, term: &mut ui::Term) {
    let mut choices: Vec<Choice> = EFFORTS.iter().map(|e| Choice::new(*e, "")).collect();
    choices.push(Choice::new("default", "the model's own default"));
    let current = match agent.effort.as_deref() {
        None => Some(EFFORTS.len()),
        Some(e) => EFFORTS.iter().position(|x| *x == e),
    };
    let keys = term.keys.as_mut().expect("keys");
    if let Some(i) = menu::pick(keys, Picker::new("Effort", choices, current), term.color) {
        agent.effort = EFFORTS.get(i).map(|e| e.to_string());
    }
    note(
        term,
        &format!(
            "effort: {} for this session",
            agent.effort.as_deref().unwrap_or("the model's default")
        ),
    );
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
    fn model_choices_start_on_the_model_in_use() {
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
        let (c, at) = model_choices(&lists, "anthropic/claude-opus-5-5");
        assert_eq!(at, 1);
        assert_eq!(c[1].detail, "Anthropic · in use");
        assert_eq!(c[2].detail, "local (OpenAI-compatible)");
        let (c, at) = model_choices(&lists, "openai/gpt-5");
        assert_eq!((at, c.len()), (0, 4));
        assert_eq!(
            (c[0].label.as_str(), c[0].detail.as_str()),
            ("openai/gpt-5", "in use")
        );
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
