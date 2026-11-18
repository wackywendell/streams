//! Validated current topology and ordinary metadata, without pair proofs.
use super::{FrontendError, HerdrInventoryPane, HerdrTopology};
use serde_json::Value;
use std::collections::BTreeMap;
type Tokens = BTreeMap<String, String>;

/// Validated topology and its tokens cannot be changed independently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrNativeSnapshot {
    raw: Value,
    topology: HerdrTopology,
    pane_tokens: BTreeMap<String, Tokens>,
    workspace_tokens: BTreeMap<String, Tokens>,
    pane_labels: BTreeMap<String, String>,
}

impl HerdrNativeSnapshot {
    pub fn parse(value: &Value) -> Result<Self, FrontendError> {
        let topology = HerdrTopology::parse(value)?;
        let mut pane_tokens = BTreeMap::new();
        let mut workspace_tokens = BTreeMap::new();
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
            raw: value.clone(),
            topology,
            pane_tokens,
            workspace_tokens,
            pane_labels,
        })
    }

    pub fn raw(&self) -> &Value {
        &self.raw
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

    pub fn first_pane(&self, pane: &HerdrInventoryPane) -> bool {
        self.topology
            .layouts
            .iter()
            .find(|l| l.tab_id == pane.pane.tab_id)
            .and_then(|l| l.panes.first())
            == Some(&pane.pane.id)
    }
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
        assert!(!parsed.first_pane(&parsed.topology().panes[0]));
        assert!(parsed.first_pane(&parsed.topology().panes[1]));
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
