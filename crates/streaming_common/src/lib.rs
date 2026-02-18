use bytes::{Buf, BufMut, Bytes, BytesMut};

pub const DATA_BUFF_SIZE: usize = 1024;

#[derive(Debug, Eq, PartialEq)]
pub struct EncodedAudioPacket {
    pub marker: bool,
    pub seq: u64,

    pub items: u16,
    pub data: [u8; DATA_BUFF_SIZE],
}

impl EncodedAudioPacket {
    pub fn new(in_data: &[u8]) -> Self {
        if in_data.len() > DATA_BUFF_SIZE {
            panic!("Input is too large");
        }

        let mut out_data = [0_u8; DATA_BUFF_SIZE];
        in_data
            .iter()
            .zip(out_data.iter_mut())
            .for_each(|(sample, out)| *out = *sample);

        EncodedAudioPacket { 
            marker: false,
            seq: 0,
            items: in_data.len() as u16,
            data: out_data,
        }
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.data[..self.items as usize]
    }
    
    pub fn as_slice_mut(&mut self) -> &[u8] {
        &mut self.data[..self.items as usize]
    }
}


impl EncodedAudioPacket {
    pub fn to_bytes(&self, buf: &mut BytesMut) {
        buf.put_u8(self.marker as u8);
        buf.put_u64_le(self.seq);
        buf.put_u16_le(self.items);

        buf.put(&self.data[..self.items as usize]);
    }

    pub fn parse(mut bytes: Bytes) -> Self {
        let marker = bytes.get_u8() == 1;
        let seq = bytes.get_u64_le();
        let items = bytes.get_u16_le();

        let mut data = [0_u8; DATA_BUFF_SIZE];
        if items > 0 {
            bytes.copy_to_slice(&mut data[..items as usize]);
        }

        Self { marker, seq, data, items }
    }
}

#[derive(Debug)]
pub enum UDPPacketType {
    Voice(EncodedAudioPacket),
    Stream(EncodedAudioPacket),
    Ping,
    Pong,
}

impl UDPPacketType {
    pub fn from_byte(ty: u8, bytes: Bytes) -> Self {
        match ty {
            0 => UDPPacketType::Voice(EncodedAudioPacket::parse(bytes)),
            1 => UDPPacketType::Stream(EncodedAudioPacket::parse(bytes)),
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
            UDPPacketType::Ping => {},
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
