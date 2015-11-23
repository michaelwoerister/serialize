
//! A serialization framework supporting the following features:
//!
//! - Arbitrary types can implement the `Encodable` and `Decodable` traits
//!   to support serialization
//! - Serialization can be "context-aware", that is, `Encodables` and
//!   `Decodables` can specify what kind of context they need to be serialized
//!   or deserialized. For example, certain values may always need to be
//!   interned at runtime. Such a value would implement `Decodable` with a
//!   context from which the needed interner is reachable.
//! - The framework supports "objects", that is, types can implement the
//!   `EncodableObject` and `DecodableObject` traits, which will make their
//!   actual data only be emitted once within some encoded dataset. Other
//!   `Encodables` only store a reference to where the objects data is located.
//! - `Encodables` do not need to know there encoded size beforehand and can
//!   indeed be of variable size. This allows for space-efficient encodings
//!   like using the LEB128 format for integers.
//! - Encoding and decoding generally does not need to do dynamic memory
//!   allocation.
//! - The API is designed in a way that should allow for easily auto-generating
//!   serialization and deserialization code.
//! - Completely safe implementation (no `unsafe` used).


use std::collections::HashMap;
use std::collections::hash_map::Entry::{Vacant, Occupied};


//=-----------------------------------------------------------------------------
// ENCODING
//=-----------------------------------------------------------------------------

#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub struct ObjectUid(u64);
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub struct ObjectTableIndex(u32);

/// Something that can be encoded given a certain context ECX.
pub trait Encodable<ECX> {
    fn encode<'ecx, 'encodable>(&'encodable self,
                                session: &mut EncodingContext<'ecx, ECX>)
        where 'encodable: 'ecx;
}

/// Values implementing this trait will only be emitted once during an
/// EncodingSession. Other values referencing them only store an address where
/// actual object can be found within the encoded data.
pub trait EncodableObject<ECX> : Encodable<ECX> {
    fn object_uid(&self) -> ObjectUid;

    fn encode_contents<'encodable, 'ecx>(&'encodable self,
                                         session: &mut EncodingContext<'ecx, ECX>)
        where 'encodable: 'ecx;
}

pub trait Encoder {
    fn position(&self) -> u64;
    fn finalize(self: Box<Self>, four_cc: [u8; 4], object_table_address: u64);

    fn emit_u32(&mut self, value: u32);
    fn emit_u64(&mut self, value: u64);
    // TODO: Add emit_* methods of other primitive and composite types
}

pub struct EncodingContext<'ctx, ECX: 'ctx> {
    encoder: &'ctx mut Encoder,
    object_table_indices: &'ctx mut HashMap<ObjectUid, ObjectTableIndex>,
    object_table: &'ctx mut Vec<u64>,
    delayed_writes: &'ctx mut Vec<(&'ctx EncodableObject<ECX>, ObjectTableIndex)>,
    pub extra: &'ctx mut ECX
}

impl<'sess, ECX: 'sess> EncodingContext<'sess, ECX> {

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

    fn get_object_table_index(&mut self, object_uid: ObjectUid) -> (ObjectTableIndex, bool) {
        match self.object_table_indices.entry(object_uid) {
            Occupied(occupied) => (*occupied.get(), false),
            Vacant(vacant) => {
                // TODO: make this a safe conversion
                let index = ObjectTableIndex(self.object_table.len() as u32);
                vacant.insert(index);
                self.object_table.push(u64::max_value());
                (index, true)
            }
        }
    }

    fn write_enqueued_objects(&mut self) {
        loop {
            // encoding objects might add more to this queue, so we can't do
            // this in a for loop
            let item = self.delayed_writes.pop();

            match item {
                Some((object, object_table_index)) => {
                    let position = self.encoder.position();
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
        self.delayed_writes.push((encodable, object_table_index));
    }
}

pub struct EncodingSession<ECX> {
    encoder: Box<Encoder>,
    object_table_indices: HashMap<ObjectUid, ObjectTableIndex>,
    object_table: Vec<u64>,
    pub context: ECX
}

impl<ECX> EncodingSession<ECX> {

    pub fn new<E: Encoder+'static>(encoder: E, ecx: ECX) -> EncodingSession<ECX> {
        EncodingSession {
            encoder: Box::new(encoder),
            object_table_indices: HashMap::new(),
            object_table: Vec::new(),
            context: ecx
        }
    }

    pub fn encode<T: Encodable<ECX>>(&mut self, encodable: &T) {
        let mut delayed_writes = Vec::new();

        let mut context = EncodingContext {
            encoder: &mut *self.encoder,
            object_table_indices: &mut self.object_table_indices,
            object_table: &mut self.object_table,
            delayed_writes: &mut delayed_writes,
            extra: &mut self.context,
        };

        encodable.encode(&mut context);
        context.write_enqueued_objects();
    }

    pub fn finalize(mut self, four_cc: [u8; 4]) {
        let object_table_address = self.encoder.position();

        for object_table_entry in self.object_table {
            self.encoder.emit_u64(object_table_entry);
        }

        self.encoder.finalize(four_cc, object_table_address);
    }
}


//=-----------------------------------------------------------------------------
// DECODING
//=-----------------------------------------------------------------------------
// TODO: The decoding API is still in flux.

pub trait Decodable<DCX> {
    fn decode(context: &mut DecodingContext<DCX>) -> Self;
}

pub trait DecodableObject<DCX> : Decodable<DCX> {
    fn decode_contents(context: &mut DecodingContext<DCX>) -> Self;
}

pub trait Decoder {
    fn set_position(&mut self, position: u64);
    fn position(&self) -> u64;
    fn read_u32(&mut self) -> u32;
    fn read_u64(&mut self) -> u64;
}

pub struct DecodingContext<'ctx, DCX> {
    decoder: &'ctx mut Decoder,
    object_table: Vec<u64>,
    pub extra: DCX,
}

impl<'ctx, DCX> DecodingContext<'ctx, DCX> {

    pub fn decoder(&mut self) -> &mut Decoder {
        self.decoder
    }

    pub fn decode_object<T:DecodableObject<DCX>>(&mut self) -> T {
        let object_table_index = self.decoder.read_u32();

        let address = self.object_table[object_table_index as usize];

        let current_position = self.decoder.position();
        self.decoder.set_position(address);

        let object = DecodableObject::decode_contents(self);

        self.decoder.set_position(current_position);

        object
    }
}


pub struct DecodingSession<'ctx, DCX> {
    context: DecodingContext<'ctx, DCX>
}

impl<'ctx, DCX> DecodingSession<'ctx, DCX> {

    pub fn decode<T: Decodable<DCX>>(&mut self) -> T {
        Decodable::decode(&mut self.context)
    }
}


//=-----------------------------------------------------------------------------
// TEST
//=-----------------------------------------------------------------------------

struct Ast {
    id: u64
}

impl<ECX> Encodable<ECX> for u64 {
    fn encode<'ecx, 'a: 'ecx>(&'a self, ecx: &mut EncodingContext<'ecx, ECX>) {
        ecx.encoder().emit_u64(*self)
    }
}

impl<DCX> Decodable<DCX> for u64 {
    fn decode(context: &mut DecodingContext<DCX>) -> u64 {
        context.decoder().read_u64()
    }
}

impl<ECX> Encodable<ECX> for Ast {
    fn encode<'ecx, 'a: 'ecx>(&'a self, ecx: &mut EncodingContext<'ecx, ECX>) {
        Encodable::encode(&self.id, ecx)
    }
}

impl<DCX> Decodable<DCX> for Ast {
    fn decode(context: &mut DecodingContext<DCX>) -> Ast {
        Ast { id: Decodable::decode(context) }
    }
}

impl<T: Encodable<ECX>, ECX> Encodable<ECX> for Option<T> {
    fn encode<'ecx, 'a: 'ecx>(&'a self, ecx: &mut EncodingContext<'ecx, ECX>) {
        match *self {
            None => {
                ecx.encoder().emit_u32(0)
            }
            Some(ref value) => {
                ecx.encoder.emit_u32(1);
                Encodable::encode(value, ecx)
            }
        }
    }
}

impl<T: Decodable<DCX>, DCX> Decodable<DCX> for Option<T> {
    fn decode(context: &mut DecodingContext<DCX>) -> Option<T> {
        let disr = context.decoder().read_u32();
        if disr == 0 {
            None
        }
        else {
            Some(Decodable::decode(context))
        }
    }
}

struct Ty<'tcx> {
    id: u64,
    ast: Ast,
    sub_ty: Option<&'tcx Ty<'tcx>>
}

trait TyRestoreContext<'tcx> {
    fn create_interned(&self, id: u64, ast: Ast, sub_ty: Option<&'tcx Ty<'tcx>>) -> &'tcx Ty<'tcx>;
}


impl<'tcx, DCX: TyRestoreContext<'tcx>> Decodable<DCX> for &'tcx Ty<'tcx> {
    fn decode(context: &mut DecodingContext<DCX>) -> &'tcx Ty<'tcx> {
        let id = Decodable::decode(context);
        let ast = Decodable::decode(context);
        let sub_ty = Decodable::decode(context);

        context.extra.create_interned(id, ast, sub_ty)
    }
}

impl<'tcx, DCX: TyRestoreContext<'tcx>> Decodable<DCX> for Ty<'tcx> {
    fn decode(context: &mut DecodingContext<DCX>) -> Ty<'tcx> {
        let id = Decodable::decode(context);
        let ast = Decodable::decode(context);
        let sub_ty = Decodable::decode(context);

        Ty {
            id: id,
            ast: ast,
            sub_ty: sub_ty
        }
    }
}
