use pumpkin_protocol::{
    ClientPacket, ServerPacket, VarInt,
    java::{
        client::play::{CParticle, CUpdateEntityPos, CUpdateEntityPosRot, CUpdateEntityRot},
        server::play::SConfirmTeleport,
    },
};
use pumpkin_util::{math::vector3::Vector3, version::JavaMinecraftVersion};

const VERSION: JavaMinecraftVersion = JavaMinecraftVersion::V_26_2;

// Fixtures follow the official 26.2 ClientboundMoveEntityPacket codecs.
#[test]
fn native_movement_places_ground_flag_after_delta_and_rotation()
-> Result<(), Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    CUpdateEntityPos::new(VarInt(7), Vector3::new(1, -2, 3), true)
        .write_packet_data(&mut bytes, &VERSION)?;
    assert_eq!(bytes, [7, 0, 1, 255, 254, 0, 3, 1]);
    let mut input = bytes.as_slice();
    assert!(CUpdateEntityPos::read(&mut input, &VERSION)?.on_ground);
    assert!(input.is_empty());

    bytes.clear();
    CUpdateEntityPosRot::new(VarInt(7), Vector3::new(1, -2, 3), 90, 45, true)
        .write_packet_data(&mut bytes, &VERSION)?;
    assert_eq!(bytes, [7, 0, 1, 255, 254, 0, 3, 90, 45, 1]);
    bytes.clear();
    CUpdateEntityRot::new(VarInt(7), 90, 45, true).write_packet_data(&mut bytes, &VERSION)?;
    assert_eq!(bytes, [7, 90, 45, 1]);
    Ok(())
}

#[test]
fn native_teleport_acknowledgement_contains_only_id() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = [0xac, 0x02].as_slice();
    let packet = SConfirmTeleport::read(&mut input, &VERSION)?;
    assert_eq!(packet.teleport_id.0, 300);
    assert!(input.is_empty());
    Ok(())
}

#[test]
fn native_particles_put_int_count_before_particle_options() -> Result<(), Box<dyn std::error::Error>>
{
    let packet = CParticle {
        force_spawn: false,
        important: true,
        position: Vector3::new(0.0, 0.0, 0.0),
        offset: Vector3::new(0.0, 0.0, 0.0),
        max_speed: 0.0,
        particle_count: 300,
        particle_id: VarInt(7),
        data: &[],
    };
    let mut bytes = Vec::new();
    packet.write_packet_data(&mut bytes, &VERSION)?;
    let mut expected = vec![1, 0];
    expected.extend_from_slice(&[0; 40]);
    expected.extend_from_slice(&300i32.to_be_bytes());
    expected.push(7);
    assert_eq!(bytes, expected);
    Ok(())
}

#[test]
fn native_copper_golem_and_weathering_serializers_are_distinct() {
    use pumpkin_data::meta_data_type::MetaDataType;
    assert_eq!(MetaDataType::COPPER_GOLEM_STATE.id, 37);
    assert_eq!(MetaDataType::WEATHERING_COPPER_STATE.id, 38);
}
