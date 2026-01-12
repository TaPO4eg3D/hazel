use bytes::{Buf, Bytes, BytesMut};

struct OpusPacket {

}

pub enum UDPPacketType {
    Opus(Bytes),
    Ack,
}

impl UDPPacketType {
    pub fn from_byte(ty: u8, bytes: Bytes) -> Self {
        match ty {
            0 => UDPPacketType::Opus(bytes),
            1 => UDPPacketType::Ack,
            _ => todo!(),
        }
    }

    pub fn to_byte(&self) -> u8 {
        match self {
            UDPPacketType::Opus(_) => 1,
            UDPPacketType::Ack => 1,
        }
    }

}

pub struct UDPPacket {
    pub seq: u16,
    pub user_id: u32,

    pub payload: UDPPacketType,
}

impl UDPPacket {
    pub fn parse(buf: &mut BytesMut) -> Self {
        let ty = buf.get_u8();
        let seq = buf.get_u16_le();
        let user_id = buf.get_u32_le();

        let payload_len = buf.remaining();
        let payload = buf.copy_to_bytes(payload_len);

        Self {
            seq,
            user_id,
            payload: UDPPacketType::from_byte(ty, payload),
        }
    }
}


