use std::ops::{Deref, DerefMut};

use bytes::{Buf, BufMut, Bytes, BytesMut};

pub const AUDIO_BUFF_SIZE: usize = 1024;

pub trait IntoUDPPayload {
    const TAG: u8;

    fn get_tag(&self) -> u8 {
        Self::TAG
    }

    fn to_bytes(&self, buf: &mut BytesMut);
}

#[derive(Debug, Eq, PartialEq)]
pub struct EncodedAudioPacket {
    pub marker: bool,
    pub seq: u64,

    pub items: u16,
    pub data: [u8; AUDIO_BUFF_SIZE],
}

impl Default for EncodedAudioPacket {
    fn default() -> Self {
        Self {
            marker: false,
            seq: 0,
            items: 0,
            data: [0_u8; AUDIO_BUFF_SIZE],
        }
    }
}

impl EncodedAudioPacket {
    pub fn new(in_data: &[u8]) -> Self {
        if in_data.len() > AUDIO_BUFF_SIZE {
            panic!("Input is too large");
        }

        let mut out_data = [0_u8; AUDIO_BUFF_SIZE];
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

    pub fn marker() -> Self {
        let mut item = Self::new(&[]);
        item.marker = true;

        item
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.data[..self.items as usize]
    }

    pub fn as_slice_mut(&mut self) -> &[u8] {
        &mut self.data[..self.items as usize]
    }
}

pub struct EncodedVideoFrame {
    pub seq: u64,
    pub items: u16,
    pub data: Vec<u8>,
}

impl IntoUDPPayload for EncodedAudioPacket {
    const TAG: u8 = 0;

    fn to_bytes(&self, buf: &mut BytesMut) {
        buf.put_u8(self.marker as u8);
        buf.put_u64_le(self.seq);
        buf.put_u16_le(self.items);

        buf.put(&self.data[..self.items as usize]);
    }
}

#[derive(Debug)]
pub struct Ping;
impl IntoUDPPayload for Ping {
    const TAG: u8 = 2;

    fn to_bytes(&self, _buf: &mut BytesMut) {}
}

#[derive(Debug)]
pub struct Pong;
impl IntoUDPPayload for Pong {
    const TAG: u8 = 3;

    fn to_bytes(&self, _buf: &mut BytesMut) {}
}

#[derive(Debug)]
pub struct EncodedAudioBytes<'a>(&'a mut Bytes);

impl<'a> EncodedAudioBytes<'a> {
    pub fn parse(self, packet: &mut EncodedAudioPacket) {
        let bytes = self.0;

        packet.marker = bytes.get_u8() == 1;
        packet.seq = bytes.get_u64_le();
        packet.items = bytes.get_u16_le();

        if packet.items > 0 {
            bytes.copy_to_slice(&mut packet.data[..packet.items as usize]);
        }
    }
}

#[derive(Debug)]
pub struct EncodedVideoBytes<'a>(&'a mut Bytes);

#[derive(Debug)]
pub enum UDPPayloadType<'a> {
    Audio(EncodedAudioBytes<'a>),
    Video(EncodedVideoBytes<'a>),
    Ping(Ping),
    Pong(Pong),
}

impl<'a> UDPPayloadType<'a> {
    pub fn from_byte(ty: u8, bytes: &'a mut Bytes) -> Self {
        match ty {
            EncodedAudioPacket::TAG => UDPPayloadType::Audio(EncodedAudioBytes(bytes)),
            1 => UDPPayloadType::Video(EncodedVideoBytes(bytes)),
            Ping::TAG => UDPPayloadType::Ping(Ping),
            Pong::TAG => UDPPayloadType::Pong(Pong),
            _ => unreachable!(),
        }
    }
}

#[derive(Debug)]
pub struct UDPPacket<'a> {
    pub user_id: i32,
    pub payload: UDPPayloadType<'a>,
}

impl<'a> UDPPacket<'a> {
    pub fn parse(buf: &'a mut Bytes) -> Self {
        let ty = buf.get_u8();
        let user_id = buf.get_i32_le();

        Self {
            user_id,
            payload: UDPPayloadType::from_byte(ty, buf),
        }
    }
}

pub fn to_udp_packet_bytes(buf: &mut BytesMut, user_id: i32, payload: &impl IntoUDPPayload) {
    buf.put_u8(payload.get_tag());
    buf.put_i32_le(user_id);

    payload.to_bytes(buf);
}
