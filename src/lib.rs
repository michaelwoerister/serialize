
use std::collections::HashMap;
use std::collections::hash_map::Entry::{Vacant, Occupied};

#[derive(Clone, Copy, Eq, PartialEq, Hash)]
struct ObjectUid(u64);
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
struct ObjectTableIndex(u32);

pub trait Encodable<ECX> {

    fn encode<'encodable, 'sess>(&'encodable self,
                                         session: &mut EncodingSession<'sess, ECX>)
        where 'encodable: 'sess;
}

pub trait EncodableObject<ECX> : Encodable<ECX> {
    fn object_uid(&self) -> ObjectUid;
    fn encode_contents<'encodable, 'sess>(&'encodable self,
                                          session: &mut EncodingSession<'sess, ECX>)
        where 'encodable: 'sess;
}

pub trait Encoder {
    fn emit_u32(&mut self, x: u32);
    fn position(&self) -> u64;
    fn finalize(self: Box<Self>, four_cc: [u8; 4], object_table_address: u64);
}

pub struct EncodingSession<'sess, ECX: 'sess> {
    encoder: Box<Encoder>,
    object_table_indices: HashMap<ObjectUid, ObjectTableIndex>,
    object_table: Vec<u64>,
    queued_objects: Vec<(&'sess EncodableObject<ECX>, ObjectTableIndex)>,
    pub context: ECX
}

impl<'sess, ECX: 'sess> EncodingSession<'sess, ECX> {

    pub fn encoder(&mut self) -> &mut Encoder {
        &mut *self.encoder
    }

    pub fn encode_object<'object, O>(&mut self, object: &'object O)
        where O: EncodableObject<ECX>,
              'object: 'sess
    {
        let object_uid = EncodableObject::object_uid(object);
        let (object_table_index, is_new) = self.get_object_table_index(object_uid);

        self.encoder().emit_u32(object_table_index.0);

        if is_new {
            self.enqueue_object_encoding(object_table_index,
                                         object as &'sess EncodableObject<ECX>);
        }
    }

    pub fn finalize(self, four_cc: [u8; 4]) {
        let object_table_address = self.encoder.position();

        // TODO: write object table

        self.encoder.finalize(four_cc, object_table_address);
    }

    fn get_object_table_index(&mut self, object_uid: ObjectUid) -> (ObjectTableIndex, bool) {
        match self.object_table_indices.entry(object_uid) {
            Occupied(occupied) => (*occupied.get(), false),
            Vacant(vacant) => {
                // TODO: make this a safe conversion
                let index = self.object_table.len() as u32;
                self.object_table.push(u64::max_value());
                (ObjectTableIndex(index), true)
            }
        }
    }

    fn write_enqueued_objects(&mut self) {
        loop {
            // encoding objects might add more to this queue, so we can't do
            // this in a for loop
            let item = self.queued_objects.pop();

            match item {
                Some((object, object_table_index)) => {
                    let position = self.encoder.position();
                    let queue_index = self.queued_objects.len();
                    object.encode_contents(self);
                    // Now that we know the address, write it to the table
                    self.object_table[object_table_index.0 as usize] = position;
                },
                None => break,
            }
        }
    }

    fn enqueue_object_encoding(&mut self,
                               object_table_index: ObjectTableIndex,
                               encodable: &'sess EncodableObject<ECX>) {
        self.queued_objects.push((encodable, object_table_index));
    }
}

pub struct EncodingSessionRef<'sess, ECX: 'sess> {
    session: EncodingSession<'sess, ECX>
}

impl<'sess, ECX: 'sess> EncodingSessionRef<'sess, ECX> {

    pub fn new<E: Encoder+'static>(encoder: E, ecx: ECX) -> EncodingSessionRef<'sess, ECX> {
        EncodingSessionRef {
            session: EncodingSession {
                encoder: Box::new(encoder),
                object_table_indices: HashMap::new(),
                object_table: Vec::new(),
                queued_objects: Vec::new(),
                context: ecx
            }
        }
    }

    pub fn encode<'encodable: 'sess, T: Encodable<ECX>>(&mut self,
                                                        encodable: &'encodable T) {
        encodable.encode(&mut self.session);
        self.session.write_enqueued_objects();
    }
}



pub trait Decodable<DCX> {
    fn decode(session: &mut DecodingSession<DCX>) -> Self;
}

pub trait DecodableObject<DCX> : Decodable<DCX> {
    fn decode_contents(session: &mut DecodingSession<DCX>) -> Self;
}

pub trait Decoder {
    fn set_position(&mut self, position: u64);
    fn position(&self) -> u64;
    fn read_u32(&mut self) -> u32;
}

pub struct DecodingSession<D: Decoder, DCX> {
    decoder: D,
    context: DCX,
    object_table: Vec<u64>
}

impl<DCX> DecodingSession<DCX> {

    pub fn decode_object<T:DecodableObject<DCX>>(&mut self) -> T {
        let object_table_index = self.decoder.read_u32();

        let address = self.object_table[object_table_index];

        let current_position = self.decoder.position();
        self.decoder.set_position(address);

        let object = DecodableObject::decode_contents(self);

        self.decoder.set_position(current_position);

        object
    }
}


pub struct DecodingSessionRef<D: Decoder, DCX> {
    session: DecodingSession<D, DCX>
}

impl<D: Decoder, DCX> DecodingSessionRef<D, DCX> {

    pub fn decode<T: Decodable<DCX>>(&mut self) -> T {
        Decodable::decode(self.session)
    }
}
