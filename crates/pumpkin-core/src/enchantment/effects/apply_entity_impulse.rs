use std::sync::Arc;

use pumpkin_data::enchantment::LevelBasedValue;
use pumpkin_util::math::vector3::Vector3;

use super::EnchantmentEntityEffectExt;
use crate::entity::Entity;
use crate::entity::player::Player;
use crate::world::World;

/// `ApplyEntityImpulse.POST_IMPULSE_CONTEXT_RESET_GRACE_TIME_TICKS`.
const POST_IMPULSE_CONTEXT_RESET_GRACE_TIME_TICKS: i32 = 10;

/// Enchantment entity effect that applies a velocity impulse to an entity.
#[derive(Clone, Debug, PartialEq)]
pub struct ApplyEntityImpulse {
    pub direction: Vector3<f64>,
    pub coordinate_scale: Vector3<f64>,
    pub magnitude: LevelBasedValue,
}

impl ApplyEntityImpulse {
    #[must_use]
    pub const fn new(
        direction: Vector3<f64>,
        coordinate_scale: Vector3<f64>,
        magnitude: LevelBasedValue,
    ) -> Self {
        Self {
            direction,
            coordinate_scale,
            magnitude,
        }
    }

    /// Computes local coordinate rotation based on entity yaw, pitch, and local offset.
    #[must_use]
    pub fn compute_local_direction(yaw: f32, pitch: f32, local: Vector3<f64>) -> Vector3<f64> {
        let y_rot_rad = (yaw + 90.0).to_radians();
        let x_rot_rad = (-pitch).to_radians();
        let x_rot_90_rad = (-pitch + 90.0).to_radians();

        let f = f64::from(y_rot_rad.cos());
        let f1 = f64::from(y_rot_rad.sin());
        let f2 = f64::from(x_rot_rad.cos());
        let f3 = f64::from(x_rot_rad.sin());
        let f4 = f64::from(x_rot_90_rad.cos());
        let f5 = f64::from(x_rot_90_rad.sin());

        let v1 = Vector3::new(f * f2, f3, f1 * f2);
        let v2 = Vector3::new(f * f4, f5, f1 * f4);
        let v3 = v1.cross(&v2) * -1.0;

        let dx = v3.x * local.x + v2.x * local.y + v1.x * local.z;
        let dy = v3.y * local.x + v2.y * local.y + v1.y * local.z;
        let dz = v3.z * local.x + v2.z * local.y + v1.z * local.z;

        Vector3::new(dx, dy, dz)
    }

    pub fn apply(
        &self,
        world: &Arc<World>,
        enchantment_level: i32,
        _owner: Option<&Arc<Player>>,
        entity: Option<&Entity>,
    ) {
        let Some(entity) = entity else {
            return;
        };

        let yaw = entity.yaw.load();
        let pitch = entity.pitch.load();
        let local_dir = Self::compute_local_direction(yaw, pitch, self.direction);

        let scale = self.coordinate_scale;
        let mag = f64::from(self.magnitude.calculate(enchantment_level));

        let impulse = Vector3::new(
            local_dir.x * scale.x * mag,
            local_dir.y * scale.y * mag,
            local_dir.z * scale.z * mag,
        );

        if let Some(player) = world.get_player_by_id(entity.entity_id) {
            // The client owns its movement and the stored server velocity is stale. The velocity
            // packet replaces the client's motion, so stack onto the movement the client
            // last reported (keeps wind charge or earlier lunge momentum).
            entity.set_velocity(entity.movement.load() + impulse);
            player
                .living_entity
                .apply_post_impulse_grace_time(POST_IMPULSE_CONTEXT_RESET_GRACE_TIME_TICKS);
        } else {
            entity.add_velocity(impulse);
            if let Some(living_base) = world.get_entity_by_id(entity.entity_id)
                && let Some(living) = living_base.get_living_entity()
            {
                living.apply_post_impulse_grace_time(POST_IMPULSE_CONTEXT_RESET_GRACE_TIME_TICKS);
            }
        }
    }
}

impl EnchantmentEntityEffectExt for ApplyEntityImpulse {
    fn apply(
        &self,
        world: &Arc<World>,
        enchantment_level: i32,
        owner: Option<&Arc<Player>>,
        entity: Option<&Entity>,
        _position: Vector3<f64>,
    ) {
        self.apply(world, enchantment_level, owner, entity);
    }
}
