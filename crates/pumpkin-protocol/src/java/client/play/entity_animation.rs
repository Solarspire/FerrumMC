use crate::ClientPacket;
use crate::VarInt;
use crate::ser::NetworkWriteExt;
use pumpkin_data::packet::clientbound::play::ANIMATE;
use pumpkin_macros::java_packet;
use pumpkin_util::version::JavaMinecraftVersion;

/// Triggers a specific animation for an entity that is visible to the client.
///
/// This is primarily used for player-driven animations like swinging an arm
/// or showing damage, but it can apply to other entities as well.
#[java_packet(ANIMATE)]
pub struct CEntityAnimation {
    /// The Entity ID of the entity performing the animation.
    pub entity_id: VarInt,
    /// The ID of the animation to play.
    /// See the table below for standard values.
    pub animation: u8,
}

impl CEntityAnimation {
    #[must_use]
    pub const fn new(entity_id: VarInt, animation: Animation) -> Self {
        Self {
            entity_id,
            animation: animation as u8,
        }
    }
}

impl ClientPacket for CEntityAnimation {
    fn write_packet_data(
        &self,
        mut write: impl std::io::Write,
        _version: &JavaMinecraftVersion,
    ) -> Result<(), crate::ser::WritingError> {
        write.write_var_int(&self.entity_id)?;

        write.write_u8(self.animation)?;
        Ok(())
    }
}

/// Plays the swing animation of an entity's hand.
///
/// Uses the same packet as [`CEntityAnimation`].
pub struct CSwingArm {
    pub entity_id: VarInt,
    pub off_hand: bool,
}

impl CSwingArm {
    #[must_use]
    pub const fn new(entity_id: VarInt, off_hand: bool) -> Self {
        Self {
            entity_id,
            off_hand,
        }
    }
}

impl crate::packet::MultiVersionJavaPacket for CSwingArm {
    fn to_id(version: JavaMinecraftVersion) -> i32 {
        ANIMATE.to_id(version)
    }
}

impl ClientPacket for CSwingArm {
    fn write_packet_data(
        &self,
        mut write: impl std::io::Write,
        _version: &JavaMinecraftVersion,
    ) -> Result<(), crate::ser::WritingError> {
        write.write_var_int(&self.entity_id)?;
        if self.off_hand {
            write.write_u8(Animation::SwingOffhand as u8)?;
        } else {
            write.write_u8(Animation::SwingMainArm as u8)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum Animation {
    SwingMainArm,
    LeaveBed = 2,
    SwingOffhand,
    CriticalEffect,
    MagicCriticaleffect,
}
