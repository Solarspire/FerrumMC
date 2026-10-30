#[allow(clippy::wildcard_imports)]
use super::*;

impl PendingConnection {
    pub async fn handle_login_acknowledged(
        &mut self,
        server: &Server,
    ) -> Option<PacketHandlerResult> {
        debug!("Handling login acknowledgement");
        // Only valid once `CLoginSuccess` went out. Without this the claimed
        // profile from `SLoginStart` reaches the config phase, and from there the
        // play phase, without ever being authenticated.
        if !self.login_success_sent {
            self.kick(TextComponent::text("Unexpected login acknowledgement"))
                .await;
            return Some(PacketHandlerResult::Stop);
        }
        self.connection_state.store(ConnectionState::Config);
        // FerrumMC: NeoForge negotiates channels first and starts vanilla configuration on pong.
        if self.neoforge_eligible() {
            self.neoforge_step(server, serde_json::json!({ "op": "open" }))
                .await;
            return None;
        }
        self.kick(TextComponent::text(super::super::neoforge::REQUIRED_CLIENT))
            .await;
        Some(PacketHandlerResult::Stop)
    }

    pub async fn start_vanilla_configuration(&mut self, server: &Server) {
        self.send_packet_now(&server.get_branding()).await;

        if server.advanced_config.server_links.enabled {
            let mut links: Vec<Link> = Vec::new();

            let bug_report = &server.advanced_config.server_links.bug_report;
            if !bug_report.is_empty() {
                links.push(Link::new(Label::BuiltIn(LinkType::BugReport), bug_report));
            }

            let support = &server.advanced_config.server_links.support;
            if !support.is_empty() {
                links.push(Link::new(Label::BuiltIn(LinkType::Support), support));
            }

            let status = &server.advanced_config.server_links.status;
            if !status.is_empty() {
                links.push(Link::new(Label::BuiltIn(LinkType::Status), status));
            }

            let feedback = &server.advanced_config.server_links.feedback;
            if !feedback.is_empty() {
                links.push(Link::new(Label::BuiltIn(LinkType::Feedback), feedback));
            }

            let community = &server.advanced_config.server_links.community;
            if !community.is_empty() {
                links.push(Link::new(Label::BuiltIn(LinkType::Community), community));
            }

            let website = &server.advanced_config.server_links.website;
            if !website.is_empty() {
                links.push(Link::new(Label::BuiltIn(LinkType::Website), website));
            }

            let forums = &server.advanced_config.server_links.forums;
            if !forums.is_empty() {
                links.push(Link::new(Label::BuiltIn(LinkType::Forums), forums));
            }

            let news = &server.advanced_config.server_links.news;
            if !news.is_empty() {
                links.push(Link::new(Label::BuiltIn(LinkType::News), news));
            }

            let announcements = &server.advanced_config.server_links.announcements;
            if !announcements.is_empty() {
                links.push(Link::new(
                    Label::BuiltIn(LinkType::Announcements),
                    announcements,
                ));
            }

            for (key, value) in &server.advanced_config.server_links.custom {
                links.push(Link::new(
                    Label::TextComponent(TextComponent::text(key.clone()).into()),
                    value,
                ));
            }

            self.send_packet_now(&CConfigServerLinks::new(&links)).await;
        }

        let resource_config = &server.advanced_config.resource_pack.java;
        if resource_config.enabled {
            let uuid = Uuid::new_v3(&uuid::Uuid::NAMESPACE_DNS, resource_config.url.as_bytes());
            let resource_pack = CConfigAddResourcePack::new(
                &uuid,
                &resource_config.url,
                &resource_config.sha1,
                resource_config.force,
                if resource_config.prompt_message.is_empty() {
                    None
                } else {
                    Some(TextComponent::text(resource_config.prompt_message.clone()))
                },
            );

            self.send_packet_now(&resource_pack).await;
        } else {
            self.send_known_packs(server).await;
        }
        debug!("login acknowledged");
    }

    pub async fn send_known_packs(&mut self, server: &Server) {
        let features = server.get_enabled_features();
        self.send_packet_now(&CFeatureFlags::new(&features)).await;
        // FerrumMC: the bridge JVM offers its own packs and later sends the registries.
        if self.neoforge {
            // Boxed: neoforge_step can start vanilla configuration, which lands here again.
            Box::pin(self.neoforge_step(server, serde_json::json!({ "op": "select_known_packs" })))
                .await;
            return;
        }
        let version_str = self.version.load().to_string();
        let loaded_packs = server.datapack_manager.get_loaded_packs();
        let known_packs = server.get_known_packs(&version_str, &loaded_packs);
        self.send_packet_now(&CKnownPacks::new(&known_packs)).await;
    }
}
