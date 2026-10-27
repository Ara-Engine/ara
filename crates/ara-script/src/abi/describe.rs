//! Descriptors of everything that crosses the Rust/C# boundary, and the macros that
//! declare a real `#[repr(C)]` item together with its descriptor.
//!
//! The C# generator ([`crate::csgen`]) reads only these descriptors, so the two sides
//! cannot drift: any change moves [`layout_hash`](super::layout_hash), and the C#
//! runtime refuses to start against stale bindings.

/// One field of an ABI struct.
#[derive(Debug)]
pub struct AbiField {
    /// Rust field name (`snake_case`).
    pub name: &'static str,
    /// The Rust type as written (`stringify!`); the generator maps it to C#.
    pub ty: &'static str,
    /// `offset_of!` in bytes.
    pub offset: usize,
    /// Doc comment.
    pub doc: &'static str,
}

/// A `#[repr(C)]` struct shared with C#.
#[derive(Debug)]
pub struct AbiStruct {
    /// Type name; the same on both sides.
    pub name: &'static str,
    /// Doc comment.
    pub doc: &'static str,
    /// `size_of`.
    pub size: usize,
    /// `align_of`.
    pub align: usize,
    /// Fields in declaration order.
    pub fields: &'static [AbiField],
}

/// One function pointer of an ABI table.
#[derive(Debug)]
pub struct AbiFn {
    /// Rust field name (`snake_case`).
    pub name: &'static str,
    /// Doc comment.
    pub doc: &'static str,
    /// Parameter names and Rust types.
    pub params: &'static [(&'static str, &'static str)],
    /// Return type as written, or `""` for none.
    pub ret: &'static str,
}

/// A `#[repr(C)]` table of `extern "C"` function pointers.
#[derive(Debug)]
pub struct AbiTable {
    /// Type name.
    pub name: &'static str,
    /// Doc comment.
    pub doc: &'static str,
    /// Function pointers in declaration order.
    pub fns: &'static [AbiFn],
}

/// An integer constant shared with C#.
#[derive(Debug)]
pub struct AbiConst {
    /// Rust name (`SCREAMING_SNAKE`); C# uses PascalCase.
    pub name: &'static str,
    /// Rust type as written.
    pub ty: &'static str,
    /// Value.
    pub value: u64,
    /// Doc comment.
    pub doc: &'static str,
}

/// Declares `#[repr(C)]` structs plus an [`AbiStruct`] list named `$list`.
macro_rules! abi_structs {
    ($list:ident;
     $( $(#[doc = $doc:literal])* pub struct $name:ident {
         $( $(#[doc = $fdoc:literal])* pub $field:ident : $fty:ty ),* $(,)?
     } )*) => {
        $(
            $(#[doc = $doc])*
            #[repr(C)]
            #[derive(Clone, Copy, Debug)]
            pub struct $name { $( $(#[doc = $fdoc])* pub $field: $fty ),* }
        )*
        /// Descriptors of the structs declared alongside this list.
        pub const $list: &[$crate::abi::AbiStruct] = &[ $( $crate::abi::AbiStruct {
            name: stringify!($name),
            doc: concat!($($doc, "\n"),*),
            size: size_of::<$name>(),
            align: align_of::<$name>(),
            fields: &[ $( $crate::abi::AbiField {
                name: stringify!($field),
                ty: stringify!($fty),
                offset: core::mem::offset_of!($name, $field),
                doc: concat!($($fdoc, "\n"),*),
            } ),* ],
        } ),* ];
    };
}

/// Declares a `#[repr(C)]` table of function pointers plus its [`AbiTable`] named `$desc`.
macro_rules! abi_table {
    ($(#[doc = $doc:literal])* pub struct $name:ident as $desc:ident {
        $( $(#[doc = $fdoc:literal])* fn $f:ident ( $( $p:ident : $pty:ty ),* ) $(-> $ret:ty)? ; )*
    }) => {
        $(#[doc = $doc])*
        #[repr(C)]
        #[derive(Clone, Copy, Debug)]
        pub struct $name {
            $( $(#[doc = $fdoc])* pub $f: unsafe extern "C" fn($($pty),*) $(-> $ret)?, )*
        }
        impl $name {
            /// Reads a foreign table, rejecting null slots before constructing Rust function pointers.
            /// # Safety
            /// `raw` points to a readable, fully initialized table. Each non-null slot
            /// has its declared signature and remains executable for the table's lifetime.
            pub unsafe fn read_checked(raw: *const Self) -> Result<Self, &'static str> {
                $(
                    // SAFETY: the caller supplies initialized table bytes. Option<extern fn>
                    // has the nullable function-pointer representation, so zero is valid here.
                    let $f = unsafe {
                        core::ptr::addr_of!((*raw).$f)
                            .cast::<Option<unsafe extern "C" fn($($pty),*) $(-> $ret)?>>()
                            .read()
                    }.ok_or(concat!(stringify!($name), ".", stringify!($f)))?;
                )*
                Ok(Self { $($f),* })
            }
        }
        /// Descriptor of the table declared alongside it.
        pub const $desc: $crate::abi::AbiTable = $crate::abi::AbiTable {
            name: stringify!($name),
            doc: concat!($($doc, "\n"),*),
            fns: &[ $( $crate::abi::AbiFn {
                name: stringify!($f),
                doc: concat!($($fdoc, "\n"),*),
                params: &[ $( (stringify!($p), stringify!($pty)) ),* ],
                ret: { #[allow(unused_variables)] let r = ""; $( let r = stringify!($ret); )? r },
            } ),* ],
        };
    };
}

/// Declares integer constants plus an [`AbiConst`] list named `$list`.
macro_rules! abi_consts {
    ($list:ident; $( $(#[doc = $doc:literal])* pub const $name:ident : $ty:ty = $val:expr; )*) => {
        $( $(#[doc = $doc])* pub const $name: $ty = $val; )*
        /// Descriptors of the constants declared alongside this list.
        pub const $list: &[$crate::abi::AbiConst] = &[ $( $crate::abi::AbiConst {
            name: stringify!($name),
            ty: stringify!($ty),
            value: $name as u64,
            doc: concat!($($doc, "\n"),*),
        } ),* ];
    };
}

/// FNV-1a over a canonical text rendering: stable across runs and platforms.
pub(crate) struct Fnv(u64);

impl Fnv {
    pub(crate) fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    /// Mixes in one token, followed by a separator byte.
    pub(crate) fn eat(&mut self, s: &str) {
        for b in s.bytes().chain([0xffu8]) {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }

    pub(crate) fn finish(&self) -> u64 {
        self.0
    }
}
