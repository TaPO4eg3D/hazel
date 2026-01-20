use bytes::{Buf, BufMut, Bytes, BytesMut};

#[derive(Debug, Eq, PartialEq)]
pub struct FFMpegPacketPayload {
    pub pts: i64,
    pub flags: i32,

    pub data: Vec<u8>,
}

impl PartialOrd for FFMpegPacketPayload {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FFMpegPacketPayload {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.pts.cmp(&other.pts)
    }
}

impl FFMpegPacketPayload {
    pub fn to_bytes(&self, buf: &mut BytesMut) {
        buf.put_i64_le(self.pts);
        buf.put_i32_le(self.flags);

        buf.put(&self.data[..]);
    }

    pub fn parse(mut bytes: Bytes) -> Self {
        let pts = bytes.get_i64_le();
        let flags = bytes.get_i32_le();

        let data_len = bytes.remaining();
        let data = bytes.slice(0..data_len).to_vec();

        Self { pts, flags, data }
    }
}

#[derive(Debug)]
pub enum UDPPacketType {
    Voice(FFMpegPacketPayload),
    Stream(FFMpegPacketPayload),
    Ping,
    Pong,
}

impl UDPPacketType {
    pub fn from_byte(ty: u8, bytes: Bytes) -> Self {
        match ty {
            0 => UDPPacketType::Voice(FFMpegPacketPayload::parse(bytes)),
            1 => UDPPacketType::Stream(FFMpegPacketPayload::parse(bytes)),
            2 => UDPPacketType::Ping,
            3 => UDPPacketType::Pong,
            _ => todo!(),
        }
    }

    pub fn get_ty_byte(&self) -> u8 {
        match self {
            UDPPacketType::Voice(_) => 0,
            UDPPacketType::Stream(_) => 1,
            UDPPacketType::Ping => 2,
            UDPPacketType::Pong => 3,
        }
    }
}

#[derive(Debug)]
pub struct UDPPacket {
    pub user_id: i32,
    pub payload: UDPPacketType,
}

impl UDPPacket {
    pub fn to_bytes(&self, buf: &mut BytesMut) {
        let ty = self.payload.get_ty_byte();

        buf.put_u8(ty);
        buf.put_i32_le(self.user_id);

        match &self.payload {
            UDPPacketType::Voice(data) => {
                data.to_bytes(buf);
            }
            _ => todo!(),
        }
    }

    pub fn parse(buf: &mut Bytes) -> Self {
        let ty = buf.get_u8();
        let user_id = buf.get_i32_le();

        let payload_len = buf.remaining();
        let payload = buf.copy_to_bytes(payload_len);

        Self {
            user_id,
            payload: UDPPacketType::from_byte(ty, payload),
        }
    }
}
