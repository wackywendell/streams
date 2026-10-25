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
    workspace_numbers: BTreeMap<String, u64>,
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
            raw: value.clone(),
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

    /// Apply this pass's acknowledged metadata write without taking another snapshot.
    pub fn apply_workspace_tokens(
        &mut self,
        workspace: &str,
        updates: BTreeMap<String, String>,
    ) -> Result<(), FrontendError> {
        let tokens = self
            .workspace_tokens
            .get_mut(workspace)
            .ok_or_else(|| FrontendError::invalid("workspace is absent from the snapshot"))?;
        tokens.extend(updates);
        let row = self.raw["workspaces"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|row| row["workspace_id"] == workspace)
            .expect("validated workspace");
        row["tokens"] = serde_json::to_value(tokens).expect("string tokens");
        Ok(())
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
