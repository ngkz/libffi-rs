//! High layer providing automatic marshalling of Rust closures
//! as C function pointers.
//!
//! The main facility here is given by the structs
//! <code>Closure<em>N</em></code>,
//! <code>Closure<span></span>Mut<em>N</em></code>,
//! <code>Closure<span></span>Once<em>N</em></code>,
//! and <code>Closure<span></span>Owned<em>N</em></code>,
//! for natural numbers *`N`*
//! from `0` to `12` (as of
//! now). These represent C closures of *`N`* arguments, which can be
//! used to turn Rust lambdas (or in generally, anything that implements
//! `Fn` or `FnMut`) into ordinary C function pointers. For example, a
//! Rust value of type `Fn(u32, u32) -> u64` can be turned into a
//! closure of type `Closure2<u32, u32, u64>` using
//! [`Closure2::new`](struct.Closure2.html#method.new). Then a C
//! function pointer of type `extern "C" fn(u32, u32) -> u64` can be
//! borrowed from the closure and passed to C.
//!
//! The above usage case eliminates much of the boilerplate involved in
//! creating a closure as compared to the `middle` and `low` layers, but
//! at the price of flexibility. Some flexibility can be recovered by
//! manually constructing and configuring a CIF (*e.g.,* a
//! [`Cif2`](struct.Cif2.html)) and then creating the closure with
//! [`Closure2::new_with_cif`](struct.Closure2.html#method.new_with_cif).
//!
//! See the [`call`](call/index.html) submodule for a simple interface
//! to dynamic calls to C functions.
//!
//! # Examples
//!
//! Here we use [`ClosureMut1`](struct.ClosureMut1.html), which is the type
//! for creating mutable closures of one argument. We use it to turn a
//! Rust lambda into a C function pointer.
//!
//! ```
//! use libffi::high::ClosureMut1;
//!
//! let mut x = 0u64;
//! let mut f = |y: u32| { x += y as u64; x };
//!
//! let closure = ClosureMut1::new(&mut f);
//! let counter = closure.code_ptr();
//!
//! assert_eq!(5, counter(5));
//! assert_eq!(6, counter(1));
//! assert_eq!(8, counter(2));
//! ```
//!
//! Note that in the above example, `counter` is an ordinary C function
//! pointer of type `extern "C" fn(u64) -> u64`.
//!
//! Here’s an example using `ClosureOnce3` to create a closure that owns
//! a vector:
//!
//! ```
//! use libffi::high::ClosureOnce3;
//!
//! let v = vec![1, 2, 3, 4, 5];
//! let mut f = move |x: usize, y: usize, z: usize| {
//!     v[x] + v[y] + v[z]
//! };
//!
//! let closure = ClosureOnce3::new(f);
//! let call = closure.code_ptr();
//!
//! assert_eq!(12, call(2, 3, 4));
//! ```
//!
//! Invoking the closure a second time will panic.

pub use middle::{FfiAbi, ffi_abi_FFI_DEFAULT_ABI};

pub mod types;
pub use self::types::{Type, CType};

#[macro_use]
pub mod call;
pub use self::call::*;

macro_rules! define_closure_mod {
    (
        $module:ident $cif:ident
          $callback:ident $callback_mut:ident $callback_once:ident
          $closure:ident $closure_mut:ident $closure_once:ident
          $closure_owned:ident;
        $( $T:ident )*
    )
        =>
    {
        /// CIF and closure types organized by function arity.
        #[cfg_attr(feature = "clippy", allow(too_many_arguments))]
        pub mod $module {
            use std::any::Any;
            use std::marker::PhantomData;
            use std::{mem, process, ptr};
            use std::io::{self, Write};

            use super::*;
            use middle;

            /// A typed CIF, which statically tracks argument and result types.
            pub struct $cif<$( $T, )* R> {
                untyped: middle::Cif,
                _marker: PhantomData<fn($( $T, )*) -> R>,
            }

            impl<$( $T, )* R> $cif<$( $T, )* R> {
                /// Creates a new statically-typed CIF with the given argument
                /// and result types.
                #[allow(non_snake_case)]
                pub fn new($( $T: Type<$T>, )* result: Type<R>) -> Self {
                    let cif = middle::Cif::new(
                        vec![$( $T.into_middle() ),*].into_iter(),
                        result.into_middle());
                    $cif { untyped: cif, _marker: PhantomData }
                }

                /// Sets the CIF to use the given calling convention.
                pub fn set_abi(&mut self, abi: FfiAbi) {
                    self.untyped.set_abi(abi);
                }
            }

            impl<$( $T: CType, )* R: CType> $cif<$( $T, )* R> {
                /// Creates a new statically-typed CIF by reifying the
                /// argument types as `Type<T>`s.
                pub fn reify() -> Self {
                    Self::new($( $T::reify(), )* R::reify())
                }
            }

            // We use tuples of pointers to describe the arguments, and we
            // extract them by pattern matching. This assumes that a tuple
            // of pointers will be laid out packed and in order. This seems
            // to hold true right now, and I can’t think of a reason why it
            // wouldn’t be that way, but technically it may be undefined
            // behavior.

            /// The type of function called from an immutable, typed closure.
            pub type $callback<U, $( $T, )* R>
                = extern "C" fn(cif:      &::low::ffi_cif,
                                result:   &mut R,
                                args:     &($( &$T, )*),
                                userdata: &U);

            /// An immutable, typed closure with the given argument and result
            /// types.
            pub struct $closure<'a, $( $T, )* R> {
                untyped: middle::Closure<'a>,
                _marker: PhantomData<fn($( $T, )*) -> R>,
            }

            impl<'a, $($T: CType,)* R: CType> $closure<'a, $($T,)* R> {
                /// Constructs a typed closure callable from C from a
                /// Rust closure.
                pub fn new<Callback>(callback: &'a Callback) -> Self
                    where Callback: Fn($( $T, )*) -> R + 'a
                {
                    Self::new_with_cif($cif::reify(), callback)
                }
            }

            impl<'a, $( $T, )* R> $closure<'a, $( $T, )* R> {
                /// Gets the C code pointer that is used to invoke the
                /// closure.
                pub fn code_ptr(&self) -> &extern "C" fn($( $T, )*) -> R {
                    unsafe {
                        mem::transmute(self.untyped.code_ptr())
                    }
                }

                /// Constructs a typed closure callable from C from a CIF
                /// describing the calling convention for the resulting
                /// function, a callback for the function to call, and
                /// userdata to pass to the callback.
                pub fn from_parts<U>(cif: $cif<$( $T, )* R>,
                                     callback: $callback<U, $( $T, )* R>,
                                     userdata: &'a U) -> Self
                {
                    let callback: middle::Callback<U, R>
                        = unsafe { mem::transmute(callback) };
                    let closure
                        = middle::Closure::new(cif.untyped,
                                               callback,
                                               userdata);
                    $closure {
                        untyped: closure,
                        _marker: PhantomData,
                    }
                }
            }

            impl<'a, $( $T: Copy, )* R> $closure<'a, $( $T, )* R> {
                /// Constructs a typed closure callable from C from a CIF
                /// describing the calling convention for the resulting
                /// function and the Rust closure to call.
                pub fn new_with_cif<Callback>(cif: $cif<$( $T, )* R>,
                                              callback: &'a Callback) -> Self
                    where Callback: Fn($( $T, )*) -> R + 'a
                {
                    Self::from_parts(cif,
                                     Self::static_callback,
                                     callback)
                }

                #[allow(non_snake_case)]
                extern "C" fn static_callback<Callback>
                    (_cif:     &::low::ffi_cif,
                     result:   &mut R,
                     &($( &$T, )*):
                               &($( &$T, )*),
                     userdata: &Callback)
                  where Callback: Fn($( $T, )*) -> R + 'a
                {
                    abort_on_panic!("Cannot panic inside FFI callback", {
                        unsafe {
                            ptr::write(result, userdata($( $T, )*));
                        }
                    });
                }
            }

            /// The type of function called from a mutable, typed closure.
            pub type $callback_mut<U, $( $T, )* R>
                = extern "C" fn(cif:      &::low::ffi_cif,
                                result:   &mut R,
                                args:     &($( &$T, )*),
                                userdata: &mut U);

            /// A mutable, typed closure with the given argument and
            /// result types.
            pub struct $closure_mut<'a, $( $T, )* R> {
                untyped: middle::Closure<'a>,
                _marker: PhantomData<fn($( $T, )*) -> R>,
            }

            impl<'a, $($T: CType,)* R: CType>
                $closure_mut<'a, $($T,)* R>
            {
                /// Constructs a typed closure callable from C from a
                /// Rust closure.
                pub fn new<Callback>(callback: &'a mut Callback) -> Self
                    where Callback: FnMut($( $T, )*) -> R + 'a
                {
                    Self::new_with_cif($cif::reify(), callback)
                }
            }

            impl<'a, $( $T, )* R> $closure_mut<'a, $( $T, )* R> {
                /// Gets the C code pointer that is used to invoke the
                /// closure.
                pub fn code_ptr(&self) -> &extern "C" fn($( $T, )*) -> R {
                    unsafe {
                        mem::transmute(self.untyped.code_ptr())
                    }
                }

                /// Constructs a typed closure callable from C from a CIF
                /// describing the calling convention for the resulting
                /// function, a callback for the function to call, and
                /// userdata to pass to the callback.
                pub fn from_parts<U>(cif:      $cif<$( $T, )* R>,
                                     callback: $callback_mut<U, $( $T, )* R>,
                                     userdata: &'a mut U) -> Self
                {
                    let callback: middle::CallbackMut<U, R>
                        = unsafe { mem::transmute(callback) };
                    let closure
                        = middle::Closure::new_mut(cif.untyped,
                                                   callback,
                                                   userdata);
                    $closure_mut {
                        untyped: closure,
                        _marker: PhantomData,
                    }
                }
            }

            impl<'a, $( $T: Copy, )* R> $closure_mut<'a, $( $T, )* R> {
                /// Constructs a typed closure callable from C from a CIF
                /// describing the calling convention for the resulting
                /// function and the Rust closure to call.
                pub fn new_with_cif<Callback>(cif: $cif<$( $T, )* R>,
                                              callback: &'a mut Callback)
                                              -> Self
                    where Callback: FnMut($( $T, )*) -> R + 'a
                {
                    Self::from_parts(cif,
                                     Self::static_callback,
                                     callback)
                }

                #[allow(non_snake_case)]
                extern "C" fn static_callback<Callback>
                    (_cif:     &::low::ffi_cif,
                     result:   &mut R,
                     &($( &$T, )*):
                               &($( &$T, )*),
                     userdata: &mut Callback)
                  where Callback: FnMut($( $T, )*) -> R + 'a
                {
                    abort_on_panic!("Cannot panic inside FFI callback", {
                        unsafe {
                            ptr::write(result, userdata($( $T, )*));
                        }
                    });
                }
            }

            /// The type of function called from a one-shot, typed closure.
            pub type $callback_once<U, $( $T, )* R>
                = $callback_mut<Option<U>, $( $T, )* R>;

            /// A one-shot, typed closure with the given argument and
            /// result types.
            pub struct $closure_once<$( $T, )* R> {
                untyped: middle::ClosureOnce,
                _marker: PhantomData<fn($( $T, )*) -> R>,
            }

            impl<$($T: CType,)* R: CType> $closure_once<$($T,)* R> {
                /// Constructs a typed closure callable from C from a
                /// Rust closure.
                pub fn new<Callback>(callback: Callback) -> Self
                    where Callback: FnOnce($( $T, )*) -> R + Any
                {
                    Self::new_with_cif($cif::reify(), callback)
                }
            }

            impl<$( $T: Copy, )* R> $closure_once<$( $T, )* R> {
                /// Constructs a one-shot closure callable from C from a CIF
                /// describing the calling convention for the resulting
                /// function and the Rust closure to call.
                pub fn new_with_cif<Callback>(cif: $cif<$( $T, )* R>,
                                              callback: Callback) -> Self
                    where Callback: FnOnce($( $T, )*) -> R + Any
                {
                    Self::from_parts(cif,
                                     Self::static_callback,
                                     callback)
                }

                #[allow(non_snake_case)]
                extern "C" fn static_callback<Callback>
                    (_cif:     &::low::ffi_cif,
                     result:   &mut R,
                     &($( &$T, )*):
                               &($( &$T, )*),
                     userdata: &mut Option<Callback>)
                  where Callback: FnOnce($( $T, )*) -> R
                {
                    if let Some(userdata) = userdata.take() {
                        abort_on_panic!("Cannot panic inside FFI callback", {
                            unsafe {
                                ptr::write(result, userdata($( $T, )*));
                            }
                        });
                    } else {
                        // There is probably a better way to abort here.
                        let _ =
                            io::stderr().write(b"FnOnce closure already used");
                        process::exit(2);
                    }
                }
            }

            impl<$( $T, )* R> $closure_once<$( $T, )* R> {
                /// Gets the C code pointer that is used to invoke the
                /// closure.
                pub fn code_ptr(&self) -> &extern "C" fn($( $T, )*) -> R {
                    unsafe {
                        mem::transmute(self.untyped.code_ptr())
                    }
                }

                /// Constructs a one-shot closure callable from C from a CIF
                /// describing the calling convention for the resulting
                /// function, a callback for the function to call, and
                /// userdata to pass to the callback.
                pub fn from_parts<U: Any>(
                    cif:      $cif<$( $T, )* R>,
                    callback: $callback_once<U, $( $T, )* R>,
                    userdata: U)
                    -> Self
                {
                    let callback: middle::CallbackOnce<U, R>
                        = unsafe { mem::transmute(callback) };
                    let closure
                        = middle::ClosureOnce::new(cif.untyped,
                                                   callback,
                                                   userdata);
                    $closure_once {
                        untyped: closure,
                        _marker: PhantomData,
                    }
                }
            }

            /// An owned, typed closure with the given argument and
            /// result types.
            pub struct $closure_owned<$( $T, )* R> {
                untyped: middle::ClosureOwned,
                _marker: PhantomData<fn($( $T, )*) -> R>,
            }

            impl<$($T: CType,)* R: CType> $closure_owned<$($T,)* R> {
                /// Constructs a typed closure callable from C from a
                /// Rust closure.
                pub fn new<Callback>(callback: Callback) -> Self
                    where Callback: FnMut($( $T, )*) -> R + Any
                {
                    Self::new_with_cif($cif::reify(), callback)
                }
            }

            impl<$( $T: Copy, )* R> $closure_owned<$( $T, )* R> {
                /// Constructs a typed closure callable from C from a CIF
                /// describing the calling convention for the resulting
                /// function and the Rust closure to call.
                pub fn new_with_cif<Callback>(cif: $cif<$( $T, )* R>,
                                              callback: Callback) -> Self
                    where Callback: FnMut($( $T, )*) -> R + Any
                {
                    Self::from_parts(cif,
                                     Self::static_callback,
                                     callback)
                }

                #[allow(non_snake_case)]
                extern "C" fn static_callback<Callback>
                    (_cif:     &::low::ffi_cif,
                     result:   &mut R,
                     &($( &$T, )*):
                               &($( &$T, )*),
                     userdata: &mut Callback)
                  where Callback: FnMut($( $T, )*) -> R
                {
                    abort_on_panic!("Cannot panic inside FFI callback", {
                        unsafe {
                            ptr::write(result, userdata($( $T, )*));
                        }
                    });
                }
            }

            impl<$( $T, )* R> $closure_owned<$( $T, )* R> {
                /// Gets the C code pointer that is used to invoke the
                /// closure.
                pub fn code_ptr(&self) -> &extern "C" fn($( $T, )*) -> R {
                    unsafe {
                        mem::transmute(self.untyped.code_ptr())
                    }
                }

                /// Constructs a typed closure callable from C from a CIF
                /// describing the calling convention for the resulting
                /// function, a callback for the function to call, and
                /// userdata to pass to the callback.
                pub fn from_parts<U: Any>(
                    cif:      $cif<$( $T, )* R>,
                    callback: $callback_mut<U, $( $T, )* R>,
                    userdata: U)
                    -> Self
                {
                    let callback: middle::CallbackMut<U, R>
                        = unsafe { mem::transmute(callback) };
                    let closure
                        = middle::ClosureOwned::new(cif.untyped,
                                                    callback,
                                                    userdata);
                    $closure_owned {
                        untyped: closure,
                        _marker: PhantomData,
                    }
                }
            }
        }

        pub use self::$module::*;
    }
}

define_closure_mod!(arity0 Cif0
                    Callback0 CallbackMut0 CallbackOnce0
                    Closure0 ClosureMut0 ClosureOnce0 ClosureOwned0;
                    );
define_closure_mod!(arity1 Cif1
                    Callback1 CallbackMut1 CallbackOnce1
                    Closure1 ClosureMut1 ClosureOnce1 ClosureOwned1;
                    A);
define_closure_mod!(arity2 Cif2
                    Callback2 CallbackMut2 CallbackOnce2
                    Closure2 ClosureMut2 ClosureOnce2 ClosureOwned2;
                    A B);
define_closure_mod!(arity3 Cif3
                    Callback3 CallbackMut3 CallbackOnce3
                    Closure3 ClosureMut3 ClosureOnce3 ClosureOwned3;
                    A B C);
define_closure_mod!(arity4 Cif4
                    Callback4 CallbackMut4 CallbackOnce4
                    Closure4 ClosureMut4 ClosureOnce4 ClosureOwned4;
                    A B C D);
define_closure_mod!(arity5 Cif5
                    Callback5 CallbackMut5 CallbackOnce5
                    Closure5 ClosureMut5 ClosureOnce5 ClosureOwned5;
                    A B C D E);
define_closure_mod!(arity6 Cif6
                    Callback6 CallbackMut6 CallbackOnce6
                    Closure6 ClosureMut6 ClosureOnce6 ClosureOwned6;
                    A B C D E F);
define_closure_mod!(arity7 Cif7
                    Callback7 CallbackMut7 CallbackOnce7
                    Closure7 ClosureMut7 ClosureOnce7 ClosureOwned7;
                    A B C D E F G);
define_closure_mod!(arity8 Cif8
                    Callback8 CallbackMut8 CallbackOnce8
                    Closure8 ClosureMut8 ClosureOnce8 ClosureOwned8;
                    A B C D E F G H);
define_closure_mod!(arity9 Cif9
                    Callback9 CallbackMut9 CallbackOnce9
                    Closure9 ClosureMut9 ClosureOnce9 ClosureOwned9;
                    A B C D E F G H I);
define_closure_mod!(arity10 Cif10
                    Callback10 CallbackMut10 CallbackOnce10
                    Closure10 ClosureMut10 ClosureOnce10 ClosureOwned10;
                    A B C D E F G H I J);
define_closure_mod!(arity11 Cif11
                    Callback11 CallbackMut11 CallbackOnce11
                    Closure11 ClosureMut11 ClosureOnce11 ClosureOwned11;
                    A B C D E F G H I J K);
define_closure_mod!(arity12 Cif12
                    Callback12 CallbackMut12 CallbackOnce12
                    Closure12 ClosureMut12 ClosureOnce12 ClosureOwned12;
                    A B C D E F G H I J K L);

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn new_with_cif() {
        let x: u64 = 1;
        let f = |y: u64, z: u64| x + y + z;

        let type_   = u64::reify();
        let cif     = Cif2::new(type_.clone(), type_.clone(), type_.clone());
        let closure = Closure2::new_with_cif(cif, &f);

        assert_eq!(12, closure.code_ptr()(5, 6));
    }

    #[test]
    fn new_with_cif_mut() {
        let mut x: u64 = 0;
        let mut f = |y: u64| { x += y; x };

        let type_   = u64::reify();
        let cif     = Cif1::new(type_.clone(), type_.clone());
        let closure = ClosureMut1::new_with_cif(cif, &mut f);

        let counter = closure.code_ptr();

        assert_eq!(5, counter(5));
        assert_eq!(6, counter(1));
        assert_eq!(8, counter(2));
    }

    #[test]
    fn new_with_cif_owned() {
        let mut x: u64 = 0;
        let f = move |y: u64| { x += y; x };

        let type_   = u64::reify();
        let cif     = Cif1::new(type_.clone(), type_.clone());
        let closure = ClosureOwned1::new_with_cif(cif, f);

        let counter = closure.code_ptr();

        assert_eq!(5, counter(5));
        assert_eq!(6, counter(1));
        assert_eq!(8, counter(2));
    }

    #[test]
    fn new() {
        let x: u64 = 1;
        let f = |y: u64, z: u64| x + y + z;

        let closure = Closure2::new(&f);

        assert_eq!(12, closure.code_ptr()(5, 6));
    }

    #[test]
    fn new_mut() {
        let mut x: u64 = 0;
        let mut f = |y: u32| { x += y as u64; x };

        let closure = ClosureMut1::new(&mut f);
        let counter = closure.code_ptr();

        assert_eq!(5, counter(5));
        assert_eq!(6, counter(1));
        assert_eq!(8, counter(2));
    }

    #[test]
    fn new_owned() {
        let mut x: u64 = 0;

        let closure = ClosureOwned1::new(move |y: u32| { x += y as u64; x });
        let counter = closure.code_ptr();

        assert_eq!(5, counter(5));
        assert_eq!(6, counter(1));
        assert_eq!(8, counter(2));
    }
}
