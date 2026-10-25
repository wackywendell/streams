use super::*;

impl NativeCardEngine {
    pub(super) fn ensure_plugin(&mut self) -> Result<()> {
        let root = self.config.plugin_root.canonicalize()?;
        let manifest = root.join("streams-rust-herdr-plugin.toml");
        let listed = self.call(
            "plugin.list",
            json!({"plugin_id":PLUGIN_ID}),
            "plugin_list",
            false,
        )?;
        let plugins = listed["plugins"]
            .as_array()
            .filter(|plugins| plugins.len() <= 1)
            .ok_or_else(|| self.refuse("unexpected Streams plugin registrations"))?;
        if let Some(plugin) = plugins.first() {
            if plugin["plugin_id"] != PLUGIN_ID
                || plugin["enabled"] != true
                || plugin["plugin_root"] != path_text(&root)?
                || plugin["manifest_path"] != path_text(&manifest)?
            {
                return Err(self.refuse("conflicting Streams plugin; registration left unchanged"));
            }
        } else {
            let linked = self.call(
                "plugin.link",
                json!({"path":manifest}),
                "plugin_linked",
                true,
            )?;
            if linked["plugin"]["plugin_id"] != PLUGIN_ID
                || linked["plugin"]["plugin_root"] != path_text(&root)?
            {
                return Err(self.refuse("unexpected Streams plugin acknowledgement"));
            }
        }
        Ok(())
    }
    pub fn open_display(&mut self, mode: OpenMode) -> Result<DisplayOutcome> {
        if self.display.is_some() {
            return Ok(DisplayOutcome::Existing);
        }
        if mode == OpenMode::Automatic && !self.config.startup_automatic() {
            return Ok(DisplayOutcome::Suppressed);
        }
        let root = self.config.plugin_root.canonicalize()?;
        let launch = self.source.launch_context();
        let bounded: BTreeMap<_, _> = launch
            .environment
            .into_iter()
            .filter(|(key, _)| SOURCE_ENV_KEYS.contains(&key.as_str()))
            .collect();
        let context = LaunchedCardContext {
            scope: self.card.scope().clone(),
            owner: self.owner.clone(),
            reader: SourceLaunch {
                environment: bounded.clone(),
                cwd: launch.cwd,
            },
            lock_directory: PathBuf::from(
                self.environment
                    .get("STREAMS_CARDS_LOCK_DIR")
                    .ok_or_else(|| invalid("card lock directory is missing"))?,
            ),
            plugin_root: root.clone(),
        };
        let mut env = bounded;
        env.insert(
            "STREAMS_CARDS_CONTEXT".into(),
            serde_json::to_string(&context).map_err(|error| invalid(error.to_string()))?,
        );
        env.insert(
            "STREAMS_RUST_BIN".into(),
            path_text(&std::env::current_exe()?)?.into(),
        );
        env.insert(
            "HERDR_SOCKET_PATH".into(),
            path_text(&self.client.endpoint().socket)?.into(),
        );
        env.insert(
            "WORK_CARDS_CONFIG_PATH".into(),
            path_text(&self.config.source_path)?.into(),
        );
        self.ensure_plugin()?;
        let opened = self.call(
            "plugin.pane.open",
            json!({"plugin_id":PLUGIN_ID,"entrypoint":"card",
            "placement":"split","target_pane_id":self.owner_pane.pane.id,"direction":"down",
            "cwd":root,"focus":false,"env":env}),
            "plugin_pane_opened",
            true,
        )?;
        let pane = plugin_pane(&opened, "card")?;
        if pane.pane.tab_id != self.owner_pane.pane.tab_id
            || pane.pane.workspace_id != self.owner_pane.pane.workspace_id
            || self
                .snapshot
                .topology()
                .panes
                .iter()
                .any(|observed| observed.terminal_id == pane.terminal_id)
        {
            return Err(
                self.refuse("card launch returned an unexpected destination; it may have opened")
            );
        }
        self.display = Some(pane.clone());
        self.rename("pane", &pane.pane.id, self.card.stream().title.clone())?;
        let fraction = self.config.pane_height_fraction;
        if fraction != 0.5 {
            let target = if fraction < 0.5 {
                &self.owner_pane
            } else {
                &pane
            };
            self.call("pane.resize", json!({"pane_id":target.pane.id,
                "direction":if fraction < 0.5 { "down" } else { "up" },"amount":(0.5-fraction).abs()}), "pane_resize", true)?;
        }
        Ok(DisplayOutcome::Opened)
    }
    pub fn close_display(&mut self) -> Result<()> {
        let Some(card) = self.display.clone() else {
            return Ok(());
        };
        let reply = self.call(
            "plugin.pane.close",
            json!({"pane_id":card.pane.id}),
            "plugin_pane_closed",
            true,
        )?;
        if reply["pane_id"] != card.pane.id {
            return Err(self.refuse("close acknowledgement differs; the card may have closed"));
        }
        self.display = None;
        Ok(())
    }
    pub fn focus_display(&mut self) -> Result<()> {
        self.follow_display()?;
        let card = self
            .display
            .clone()
            .ok_or_else(|| invalid("Session's card is not observed"))?;
        let reply = self.call(
            "plugin.pane.focus",
            json!({"pane_id":card.pane.id}),
            "plugin_pane_focused",
            true,
        )?;
        if plugin_pane(&reply, "card")?.terminal_id != card.terminal_id {
            return Err(self.refuse("focus acknowledgement differs"));
        }
        Ok(())
    }
    pub fn follow_display(&mut self) -> Result<()> {
        let Some(card) = self
            .display
            .clone()
            .filter(|card| card.pane.tab_id != self.owner_pane.pane.tab_id)
        else {
            return Ok(());
        };
        let reply = self.call("pane.move", json!({"pane_id":card.pane.id,"destination":{"type":"tab",
            "tab_id":self.owner_pane.pane.tab_id,"target_pane_id":self.owner_pane.pane.id,"split":"down"},"focus":false}), "pane_move", true)?;
        let pane = pane(&reply["move_result"]["pane"])?;
        if pane.terminal_id != card.terminal_id
            || pane.pane.tab_id != self.owner_pane.pane.tab_id
            || pane.pane.workspace_id != self.owner_pane.pane.workspace_id
        {
            return Err(self.refuse(
                "move did not place the Session's card in Pi's tab; inspect before retrying",
            ));
        }
        self.display = Some(pane);
        Ok(())
    }
    pub(super) fn rename(&mut self, kind: &str, id: &str, title: String) -> Result<()> {
        config::validate_title(&title)?;
        let (method, result, id_field) = match kind {
            "workspace" => ("workspace.rename", "workspace_info", "workspace_id"),
            "tab" => ("tab.rename", "tab_info", "tab_id"),
            "pane" => ("pane.rename", "pane_info", "pane_id"),
            _ => unreachable!("native name surface"),
        };
        let reply = self.call(method, json!({id_field:id,"label":title}), result, true)?;
        if reply[kind][id_field] != id || reply[kind]["label"] != title {
            return Err(self.refuse("name acknowledgement differs; the surface may have changed"));
        }
        Ok(())
    }
}
fn pane(value: &Value) -> Result<HerdrInventoryPane> {
    Ok(HerdrInventoryPane::parse(value)?)
}
pub(super) fn plugin_pane(reply: &Value, entrypoint: &str) -> Result<HerdrInventoryPane> {
    let info = &reply["plugin_pane"];
    if info["plugin_id"] != PLUGIN_ID || info["entrypoint"] != entrypoint {
        return Err(invalid(
            "unexpected plugin pane acknowledgement; the pane may have opened",
        ));
    }
    pane(&info["pane"])
}
pub(super) fn path_text(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| invalid("native path must be UTF-8"))
}
