//! Store replayable workload recipes and admit a selected recipe as a recording.
use super::Operations;
use anyhow::Result;
use serde_json::{Value, json};

impl Operations {
    pub(super) async fn save_scenario(&self, params: Value) -> Result<Value> {
        let mut scenario = params;
        let id = scenario["id"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        scenario["id"] = json!(id);
        let store = self.engine.store().clone();
        self.metadata_work
            .run(move || {
                store.save_document("scenarios", &id, &scenario)?;
                Ok(json!({"scenario":scenario}))
            })
            .await
    }

    pub(super) async fn run_scenario(&self, params: Value) -> Result<Value> {
        let store = self.engine.store().clone();
        let scenario_id = params["scenario_id"].as_str().unwrap().to_owned();
        let mut scenario = self
            .metadata_work
            .read(move |_| store.document("scenarios", &scenario_id))
            .await?;
        scenario.as_object_mut().unwrap().remove("id");
        if let Some(name) = params.get("name") {
            scenario["name"] = name.clone();
        }
        let request = serde_json::from_value(scenario)?;
        Ok(json!({"run":self.engine.start(request).await?}))
    }
}
