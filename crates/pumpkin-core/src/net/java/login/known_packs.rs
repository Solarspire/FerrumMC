#[allow(clippy::wildcard_imports)]
use super::*;

impl PendingConnection {
    pub async fn handle_known_packs(
        &mut self,
        server: &Server,
        packs: &pumpkin_protocol::java::server::config::SKnownPacks<'_>,
    ) {
        // FerrumMC: the bridge JVM sends the registries and tags, including mod datapack entries.
        if self.neoforge {
            let packs: Vec<_> = packs
                .known_packs
                .iter()
                .map(|p| serde_json::json!({ "namespace": p.namespace, "id": p.id, "version": p.version }))
                .collect();
            self.neoforge_step(
                server,
                serde_json::json!({ "op": "known_packs", "packs": packs }),
            )
            .await;
            return;
        }
        let version = CURRENT_MC_VERSION;
        let synced = server.datapack_manager.get_all_synced_registries();
        for reg in &synced {
            self.send_packet_now(&CRegistryData::new(&reg.registry_id, &reg.registry_entries))
                .await;
        }
        let mut tags = Vec::new();
        for &key in pumpkin_data::tag::RegistryKey::NETWORK_KEYS {
            if pumpkin_data::tag::get_registry_key_tags(version, key)
                .is_some_and(|map| !map.is_empty())
            {
                tags.push(key);
            }
        }
        self.send_packet_now(&CUpdateTags::new(&tags)).await;
        self.send_packet_now(&CFinishConfig).await;
    }
}
