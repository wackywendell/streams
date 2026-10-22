//! Token-bearing native observations, not catalog or session authority.

use super::{FrontendError, HerdrInventoryPane, HerdrTopology};
use serde_json::Value;
use std::collections::BTreeMap;

// Transport marker schema, not the set of pair states.
const PAIR_KEYS: [&str; 6] = [
    "strm_agent",
    "strm_nonce",
    "strm_pi",
    "strm_card",
    "strm_role",
    "strm_native",
];
type Tokens = BTreeMap<String, String>;

/// Validated topology and its tokens cannot be changed independently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrNativeSnapshot {
    topology: HerdrTopology,
    pane_tokens: BTreeMap<String, Tokens>,
    workspace_tokens: BTreeMap<String, Tokens>,
    pane_labels: BTreeMap<String, String>,
    workspace_numbers: BTreeMap<String, u64>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PairState<'a> {
    Empty,
    CardOnly {
        card: &'a HerdrInventoryPane,
        nonce: &'a str,
    },
    Absent {
        agent: &'a HerdrInventoryPane,
        nonce: &'a str,
    },
    Paired {
        agent: &'a HerdrInventoryPane,
        card: &'a HerdrInventoryPane,
        nonce: &'a str,
    },
    Separated {
        agent: &'a HerdrInventoryPane,
        card: &'a HerdrInventoryPane,
        nonce: &'a str,
    },
}

#[derive(Clone, Copy)]
enum Role {
    Agent,
    Card,
}

impl HerdrNativeSnapshot {
    pub fn parse(value: &Value) -> Result<Self, FrontendError> {
        let topology = HerdrTopology::parse(value)?;
        let mut pane_tokens = BTreeMap::new();
        let mut workspace_tokens = BTreeMap::new();
        let mut workspace_numbers = BTreeMap::new();
        let mut pane_labels = BTreeMap::new();
        // Topology parsing already checked the inventories and their identities.
        for key in ["workspaces", "tabs"] {
            for raw in value[key].as_array().unwrap() {
                if !raw
                    .get("number")
                    .and_then(Value::as_u64)
                    .is_some_and(|n| (1..=9_007_199_254_740_991).contains(&n))
                {
                    return Err(FrontendError::invalid("invalid Herdr snapshot number"));
                }
            }
        }
        for raw in value["workspaces"].as_array().unwrap() {
            let id = raw["workspace_id"].as_str().unwrap();
            workspace_tokens.insert(id.into(), tokens(raw)?);
            workspace_numbers.insert(id.into(), raw["number"].as_u64().unwrap());
        }
        for raw in value["panes"].as_array().unwrap() {
            let id = raw["pane_id"].as_str().unwrap();
            pane_tokens.insert(id.into(), tokens(raw)?);
            match raw.get("label") {
                None | Some(Value::Null) => {}
                Some(Value::String(label)) => {
                    pane_labels.insert(id.into(), label.clone());
                }
                _ => return Err(FrontendError::invalid("invalid Herdr pane label")),
            }
        }
        Ok(Self {
            topology,
            pane_tokens,
            workspace_tokens,
            pane_labels,
            workspace_numbers,
        })
    }

    pub fn workspace_number(&self, id: &str) -> Option<u64> {
        self.workspace_numbers.get(id).copied()
    }

    pub fn tab_number(&self, id: &str) -> Option<u64> {
        self.topology
            .tabs
            .iter()
            .find(|tab| tab.id == id)
            .and_then(|tab| tab.number)
    }

    pub fn topology(&self) -> &HerdrTopology {
        &self.topology
    }

    pub fn pane_tokens(&self, pane_id: &str) -> Option<&BTreeMap<String, String>> {
        self.pane_tokens.get(pane_id)
    }

    pub fn workspace_tokens(&self, workspace_id: &str) -> Option<&BTreeMap<String, String>> {
        self.workspace_tokens.get(workspace_id)
    }

    pub fn pane_label(&self, pane_id: &str) -> Option<&str> {
        self.pane_labels.get(pane_id).map(String::as_str)
    }

    pub fn locate_terminal(&self, terminal: &str) -> Result<&HerdrInventoryPane, FrontendError> {
        self.topology
            .panes
            .iter()
            .find(|p| p.terminal_id == terminal)
            .ok_or_else(|| FrontendError::invalid("missing live Herdr terminal identity"))
    }

    pub fn first_pane(&self, pane: &HerdrInventoryPane) -> bool {
        self.topology
            .layouts
            .iter()
            .find(|l| l.tab_id == pane.pane.tab_id)
            .and_then(|l| l.panes.first())
            == Some(&pane.pane.id)
    }

    fn markers(&self, pane: &HerdrInventoryPane) -> &Tokens {
        &self.pane_tokens[&pane.pane.id]
    }

    /// Identity is supplied by the caller; native markers confer no authority.
    pub fn classify_pair(
        &self,
        identity: &str,
        owner_terminal: &str,
    ) -> Result<PairState<'_>, FrontendError> {
        let caller = self.locate_terminal(owner_terminal)?;
        let mut agent = None;
        let mut card = None;
        for pane in &self.topology.panes {
            let markers = self.markers(pane);
            if marker(markers, "strm_pi") == Some(owner_terminal)
                && marker(markers, "strm_agent") != Some(identity)
            {
                return Err(FrontendError::invalid(
                    "foreign or incomplete agent owner markers",
                ));
            }
            if marker(markers, "strm_agent") != Some(identity) {
                continue;
            }
            if !PAIR_KEYS
                .iter()
                .all(|key| marker(markers, key).is_some_and(|v| !v.is_empty()))
            {
                return Err(FrontendError::invalid("incomplete pair markers"));
            }
            let role = match marker(markers, "strm_role") {
                Some("agent") => Role::Agent,
                Some("card") => Role::Card,
                _ => return Err(FrontendError::invalid("invalid pair role")),
            };
            let slot = match role {
                Role::Agent => &mut agent,
                Role::Card => &mut card,
            };
            if slot.replace(pane).is_some() {
                return Err(FrontendError::invalid("conflicting duplicate live owners"));
            }
        }
        let caller_marked = PAIR_KEYS
            .iter()
            .any(|key| self.markers(caller).contains_key(*key));
        let Some(agent) = agent else {
            let Some(card) = card else {
                return if caller_marked {
                    Err(FrontendError::invalid("foreign or incomplete pair markers"))
                } else {
                    Ok(PairState::Empty)
                };
            };
            let c = self.markers(card);
            if caller_marked
                || c["strm_pi"] != owner_terminal
                || card.terminal_id != c["strm_card"]
                || card.pane.workspace_id != caller.pane.workspace_id
                || card.pane.tab_id != caller.pane.tab_id
            {
                return Err(FrontendError::invalid(
                    "foreign or incomplete reserved card markers",
                ));
            }
            return Ok(PairState::CardOnly {
                card,
                nonce: &c["strm_nonce"],
            });
        };
        let a = self.markers(agent);
        if self.pane_tokens.values().any(|tokens| {
            marker(tokens, "strm_pi") == Some(a["strm_pi"].as_str())
                && marker(tokens, "strm_agent") != Some(identity)
        }) {
            return Err(FrontendError::invalid("conflicting owner markers"));
        }
        if agent.terminal_id != a["strm_pi"]
            || (owner_terminal != a["strm_pi"] && owner_terminal != a["strm_card"])
        {
            return Err(FrontendError::invalid("foreign agent terminal"));
        }
        let Some(card) = card else {
            if self
                .topology
                .panes
                .iter()
                .any(|p| p.terminal_id == a["strm_card"])
            {
                return Err(FrontendError::invalid(
                    "foreign or incomplete live card markers",
                ));
            }
            return Ok(PairState::Absent {
                agent,
                nonce: &a["strm_nonce"],
            });
        };
        let c = self.markers(card);
        if card.terminal_id != a["strm_card"]
            || PAIR_KEYS
                .iter()
                .filter(|key| **key != "strm_role")
                .any(|key| c[*key] != a[*key])
        {
            return Err(FrontendError::invalid("contradictory pair markers"));
        }
        let nonce = a["strm_nonce"].as_str();
        if agent.pane.workspace_id == card.pane.workspace_id
            && agent.pane.tab_id == card.pane.tab_id
        {
            Ok(PairState::Paired { agent, card, nonce })
        } else {
            Ok(PairState::Separated { agent, card, nonce })
        }
    }
}

fn marker<'a>(tokens: &'a Tokens, key: &str) -> Option<&'a str> {
    tokens.get(key).map(String::as_str)
}

fn tokens(raw: &Value) -> Result<Tokens, FrontendError> {
    match raw.get("tokens") {
        None => Ok(BTreeMap::new()),
        Some(Value::Object(values)) => values
            .iter()
            .map(|(k, v)| {
                v.as_str()
                    .map(|v| (k.clone(), v.into()))
                    .ok_or_else(|| FrontendError::invalid("invalid Herdr token value"))
            })
            .collect(),
        _ => Err(FrontendError::invalid("invalid Herdr tokens")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snapshot() -> Value {
        json!({
            "workspaces": [{"workspace_id":"w", "label":"Workspace", "number":1}],
            "tabs": [{"workspace_id":"w", "tab_id":"t", "label":"Tab", "number":1}],
            "panes": [
                {"workspace_id":"w", "tab_id":"t", "pane_id":"a", "terminal_id":"owner"},
                {"workspace_id":"w", "tab_id":"t", "pane_id":"c", "terminal_id":"card"}
            ],
            "layouts": [{"workspace_id":"w", "tab_id":"t", "panes":[{"pane_id":"c"},{"pane_id":"a"}]}],
            "agents": []
        })
    }

    fn markers(role: &str) -> Value {
        json!({"strm_agent":"identity", "strm_nonce":" nonce ", "strm_pi":"owner",
            "strm_card":"card", "strm_role":role, "strm_native":"native"})
    }

    fn paired() -> Value {
        let mut s = snapshot();
        s["panes"][0]["tokens"] = markers("agent");
        s["panes"][1]["tokens"] = markers("card");
        s
    }

    fn rejects(s: &Value, owner: &str) {
        assert!(
            HerdrNativeSnapshot::parse(s)
                .unwrap()
                .classify_pair("identity", owner)
                .is_err()
        );
    }

    #[test]
    fn card_caller_cannot_hide_foreign_owner_markers() {
        let mut s = paired();
        s["panes"].as_array_mut().unwrap().push(json!({
            "workspace_id":"w","tab_id":"t","pane_id":"foreign","terminal_id":"other",
            "tokens":{"strm_pi":"owner","strm_agent":"foreign"}
        }));
        s["layouts"][0]["panes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"pane_id":"foreign"}));
        rejects(&s, "owner");
        rejects(&s, "card");
    }

    #[test]
    fn all_pair_states_and_borrowed_records() {
        let mut s = snapshot();
        assert_eq!(
            HerdrNativeSnapshot::parse(&s)
                .unwrap()
                .classify_pair("identity", "owner")
                .unwrap(),
            PairState::Empty
        );
        s["panes"][1]["tokens"] = markers("card");
        let parsed = HerdrNativeSnapshot::parse(&s).unwrap();
        assert_eq!(parsed.workspace_number("w"), Some(1));
        assert_eq!(parsed.tab_number("t"), Some(1));
        assert_eq!(
            parsed.classify_pair("identity", "owner").unwrap(),
            PairState::CardOnly {
                card: parsed.locate_terminal("card").unwrap(),
                nonce: " nonce "
            }
        );
        s["panes"][0]["tokens"] = markers("agent");
        let parsed = HerdrNativeSnapshot::parse(&s).unwrap();
        for owner in ["owner", "card"] {
            assert_eq!(
                parsed.classify_pair("identity", owner).unwrap(),
                PairState::Paired {
                    agent: &parsed.topology().panes[0],
                    card: &parsed.topology().panes[1],
                    nonce: " nonce "
                }
            );
        }
        s["tabs"]
            .as_array_mut()
            .unwrap()
            .push(json!({"workspace_id":"w", "tab_id":"other", "label":"Other", "number":2}));
        s["panes"][1]["tab_id"] = json!("other");
        s["layouts"][0]["panes"] = json!([{"pane_id":"a"}]);
        s["layouts"]
            .as_array_mut()
            .unwrap()
            .push(json!({"workspace_id":"w", "tab_id":"other", "panes":[{"pane_id":"c"}]}));
        assert!(matches!(
            HerdrNativeSnapshot::parse(&s)
                .unwrap()
                .classify_pair("identity", "owner"),
            Ok(PairState::Separated {
                nonce: " nonce ",
                ..
            })
        ));
        s["panes"][0].as_object_mut().unwrap().remove("tokens");
        rejects(&s, "owner"); // A reserved card must still share the caller's tab.
        s["panes"][0]["tokens"] = markers("agent");
        s["panes"].as_array_mut().unwrap().pop();
        s["layouts"][1]["panes"] = json!([]);
        assert!(matches!(
            HerdrNativeSnapshot::parse(&s)
                .unwrap()
                .classify_pair("identity", "owner"),
            Ok(PairState::Absent {
                nonce: " nonce ",
                ..
            })
        ));
    }

    #[test]
    fn partial_foreign_contradictory_and_duplicate_markers_fail_closed() {
        for key in PAIR_KEYS {
            let mut s = paired();
            s["panes"][1]["tokens"].as_object_mut().unwrap().remove(key);
            rejects(&s, "owner");
            s["panes"][1]["tokens"][key] = json!("");
            rejects(&s, "owner");
            let mut s = paired();
            s["panes"][1]["tokens"][key] = json!("different");
            rejects(&s, "owner");
        }
        let mut s = paired();
        s["panes"][1].as_object_mut().unwrap().remove("tokens");
        rejects(&s, "owner"); // Unmarked but live referenced card.
        for role in ["agent", "card"] {
            let mut s = paired();
            for pane in s["panes"].as_array_mut().unwrap() {
                pane["tokens"] = markers(role);
            }
            rejects(&s, "owner");
        }
        let mut s = snapshot();
        s["panes"][1]["tokens"] = json!({"strm_pi":"owner"});
        rejects(&s, "owner");
        for key in PAIR_KEYS {
            let mut s = snapshot();
            s["panes"][0]["tokens"] = json!({(key):""});
            rejects(&s, "owner");
        }
        rejects(&paired(), "missing");
    }

    #[test]
    fn reserved_card_and_agent_terminal_conflicts() {
        let mut s = snapshot();
        s["panes"][1]["tokens"] = markers("card");
        rejects(&s, "card");
        s["panes"][1]["tokens"]["strm_pi"] = json!("elsewhere");
        rejects(&s, "owner");
        let mut s = paired();
        s["panes"][0]["tokens"]["strm_pi"] = json!("elsewhere");
        rejects(&s, "owner");
        let mut s = paired();
        s["panes"][0]["tokens"]["strm_card"] = json!("elsewhere");
        rejects(&s, "card");
        let mut s = paired();
        s["panes"][1]["terminal_id"] = json!("wrong");
        rejects(&s, "owner");
        s["panes"][0].as_object_mut().unwrap().remove("tokens");
        rejects(&s, "owner");
    }

    #[test]
    fn token_label_and_number_validation() {
        for collection in ["panes", "workspaces"] {
            for bad in [
                Value::Null,
                json!([]),
                json!(1),
                json!("tokens"),
                json!({"x":null}),
                json!({"x":1}),
            ] {
                let mut s = snapshot();
                s[collection][0]["tokens"] = bad;
                assert!(HerdrNativeSnapshot::parse(&s).is_err());
            }
        }
        for collection in ["workspaces", "tabs"] {
            for bad in [
                Value::Null,
                json!(0),
                json!(-1),
                json!(1.5),
                json!(9_007_199_254_740_992_u64),
                json!("1"),
            ] {
                let mut s = snapshot();
                s[collection][0]["number"] = bad;
                assert!(HerdrNativeSnapshot::parse(&s).is_err());
            }
            let mut s = snapshot();
            s[collection][0].as_object_mut().unwrap().remove("number");
            assert!(HerdrNativeSnapshot::parse(&s).is_err());
            s[collection][0]["number"] = json!(9_007_199_254_740_991_u64);
            assert!(HerdrNativeSnapshot::parse(&s).is_ok());
        }
        let mut s = snapshot();
        s["panes"][0]["label"] = json!(false);
        assert!(HerdrNativeSnapshot::parse(&s).is_err());
    }

    #[test]
    fn topology_boundary_and_layout_order() {
        let mut s = snapshot();
        s["panes"][0]["label"] = json!("Agent");
        s["panes"][1]["label"] = Value::Null;
        s["workspaces"][0]["tokens"] = json!({"raw":" value\n "});
        let parsed = HerdrNativeSnapshot::parse(&s).unwrap();
        assert!(!parsed.first_pane(parsed.locate_terminal("owner").unwrap()));
        assert!(parsed.first_pane(parsed.locate_terminal("card").unwrap()));
        assert_eq!(parsed.pane_label("a"), Some("Agent"));
        assert_eq!(parsed.pane_label("c"), None);
        assert!(parsed.pane_tokens("a").unwrap().is_empty());
        assert_eq!(parsed.workspace_tokens("w").unwrap()["raw"], " value\n ");
        assert!(parsed.pane_tokens("missing").is_none());
        s["panes"][0]["workspace_id"] = json!("foreign");
        assert!(HerdrNativeSnapshot::parse(&s).is_err());
        let mut s = snapshot();
        s["layouts"][0]["panes"] = json!([{"pane_id":"a"}]);
        assert!(HerdrNativeSnapshot::parse(&s).is_err());
        let mut s = snapshot();
        s.as_object_mut().unwrap().remove("agents");
        assert!(HerdrNativeSnapshot::parse(&s).is_err());
        let mut s = snapshot();
        s["panes"][1]["terminal_id"] = json!("owner");
        assert!(HerdrNativeSnapshot::parse(&s).is_err());
    }
}
