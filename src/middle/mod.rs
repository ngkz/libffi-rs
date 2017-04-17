//! Middle layer providing a somewhat safer (but still quite unsafe)
//! API.
//!
//! The main idea of the middle layer is to wrap types
//! [`ffi_cif`](../raw/struct.ffi_cif.html) and
//! [`ffi_closure`](../raw/struct.ffi_closure.html) as
//! [`Cif`](struct.Cif.html) and [`Closure`](struct.Closure.html),
//! respectively, so that their resources are managed properly. However,
//! calling a function via a CIF or closure is still unsafe because
//! argument types aren’t checked. See the [`high`](../high/index.html)
//! layer for closures with type-checked arguments.

use std::any::Any;
use std::os::raw::c_void;
use std::marker::PhantomData;

use low;
pub use low::{Callback, CallbackMut, CodePtr,
              ffi_abi as FfiAbi, ffi_abi_FFI_DEFAULT_ABI};

#[cfg(not(feature = "unique"))]
mod util;

mod types;
pub use self::types::Type;

mod builder;
pub use self::builder::Builder;

/// Contains an untyped pointer to a function argument.
///
/// When calling a function via a [CIF](struct.Cif.html), each argument
/// must be passed as a C `void*`. Wrapping the argument in the `Arg`
/// struct accomplishes the necessary coercion.
#[derive(Clone, Debug)]
#[repr(C)]
pub struct Arg(*mut c_void);

impl Arg {
    /// Coerces an argument reference into the `Arg` type.
    ///
    /// This is used to wrap each argument pointer before passing them
    /// to [`Cif::call`](struct.Cif.html#method.call).
    pub fn new<T>(r: &T) -> Self {
        Arg(r as *const T as *mut c_void)
    }
}

/// Coerces an argument reference into the [`Arg`](struct.Arg.html)
/// type.
///
/// This is used to wrap each argument pointer before passing them
/// to [`Cif::call`](struct.Cif.html#method.call).
/// (This is the same as [`Arg::new`](struct.Arg.html#method.new)).
pub fn arg<T>(r: &T) -> Arg {
    Arg::new(r)
}

/// Describes the calling convention and types for calling a function.
///
/// This is the `middle` layer’s wrapping of the `low` and `raw` layers’
/// [`ffi_cif`](../raw/struct.ffi_cif.html). An initialized CIF contains
/// references to an array of argument types and a result type, each of
/// which may be allocated on the heap. `Cif` manages the memory of
/// those referenced objects.
///
/// Construct with [`Cif::new`](#method.new) or
/// [`Cif::from_type_array`](#method.from_type_array).
///
/// # Examples
///
/// ```
/// extern "C" fn add(x: f64, y: &f64) -> f64 {
///     return x + y;
/// }
///
/// use libffi::middle::*;
///
/// let args = vec![Type::f64(), Type::pointer()];
/// let cif = Cif::new(args.into_iter(), Type::f64());
///
/// let n = unsafe { cif.call(CodePtr(add as *mut _), &[arg(&5), arg(&&6)]) };
/// assert_eq!(11, n);
/// ```
#[derive(Debug)]
pub struct Cif {
    cif:    low::ffi_cif,
    args:   types::TypeArray,
    result: Type,
}

// To clone a Cif we need to clone the types and then make sure the new
// ffi_cif refers to the clones of the types.
impl Clone for Cif {
    fn clone(&self) -> Self {
        let mut copy = Cif {
            cif:    self.cif,
            args:   self.args.clone(),
            result: self.result.clone(),
        };

        copy.cif.arg_types = copy.args.as_raw_ptr();
        copy.cif.rtype     = copy.result.as_raw_ptr();

        copy
    }
}

impl Cif {
    /// Creates a new CIF for the given argument and result types.
    ///
    /// Takes ownership of the argument and result
    /// [`Type`](types/struct.Type.html)s, because the resulting
    /// `Cif` retains references to them.
    /// Defaults to the platform’s default calling convention; this
    /// can be adjusted using [`set_abi`](#method.set_abi).
    pub fn new<I>(args: I, result: Type) -> Self
        where I: ExactSizeIterator<Item=Type>
    {
        let nargs = args.len();
        let args = types::TypeArray::new(args);
        let mut cif: low::ffi_cif = Default::default();

        unsafe {
            low::prep_cif(&mut cif,
                          low::ffi_abi_FFI_DEFAULT_ABI,
                          nargs,
                          result.as_raw_ptr(),
                          args.as_raw_ptr())
        }.expect("low::prep_cif");

        // Note that cif retains references to args and result,
        // which is why we hold onto them here.
        Cif {
            cif:    cif,
            args:   args,
            result: result,
        }
    }

    /// Calls a function with the given arguments.
    ///
    /// In particular, this method invokes function `fun` passing it
    /// arguments `args`, and returns the result.
    ///
    /// # Safety
    ///
    /// There is no checking that the calling convention and types
    /// in the `Cif` match the actual calling convention and types of
    /// `fun`, nor that they match the types of `args`.
    pub unsafe fn call<R>(&self, fun: CodePtr, args: &[Arg]) -> R {
        use std::mem;

        assert!(self.cif.nargs as usize == args.len(),
                "Cif::call: passed wrong number of arguments");

        low::call::<R>(&self.cif as *const _ as *mut _,
                       fun,
                       mem::transmute::<*const Arg,
                                        *mut *mut c_void>(args.as_ptr()))
    }

    /// Sets the CIF to use the given calling convention.
    pub fn set_abi(&mut self, abi: FfiAbi) {
        self.cif.abi = abi;
    }

    /// Gets a raw pointer to the underlying
    /// [`ffi_cif`](../low/struct.ffi_cif.html).
    ///
    /// This can be used for passing a `middle::Cif` to functions from the
    /// [`low`](../low/index.html) and [`raw`](../raw/index.html) modules.
    pub fn as_raw_ptr(&self) -> *mut low::ffi_cif {
        &self.cif as *const _ as *mut _
    }
}

/// Represents a closure callable from C.
///
/// A libffi closure captures a `void*` (“userdata”) and passes it to a
/// callback when the code pointer (obtained via
/// [`code_ptr`](#method.code_ptr)) is invoked. Lifetype parameter `'a`
/// ensures that the closure does not outlive the userdata.
///
/// Construct with [`Closure::new`](#method.new) and
/// [`Closure::new_mut`](#method.new_mut).
///
/// # Examples
///
/// In this example we turn a Rust lambda into a C function. We first
/// define function `lambda_callback`, which will be called by libffi
/// when the closure is called. The callback function takes four
/// arguments: a CIF describing its arguments, a pointer for where to
/// store its result, a pointer to an array of pointers to its
/// arguments, and a userdata pointer. In this ase, the Rust closure
/// value `lambda` is passed as userdata to `lambda_callback`, which
/// then invokes it.
///
/// ```
/// use std::mem;
/// use std::os::raw::c_void;
///
/// use libffi::middle::*;
/// use libffi::low;
///
/// unsafe extern "C" fn lambda_callback<F: Fn(u64, u64) -> u64>(
///     _cif: &low::ffi_cif,
///     result: &mut u64,
///     args: *const *const c_void,
///     userdata: &F)
/// {
///     let args: *const &u64 = mem::transmute(args);
///     let arg1 = **args.offset(0);
///     let arg2 = **args.offset(1);
///
///     *result = userdata(arg1, arg2);
/// }
///
/// let cif = Cif::new(vec![Type::u64(), Type::u64()].into_iter(),
///                    Type::u64());
/// let lambda = |x: u64, y: u64| x + y;
/// let closure = Closure::new(cif, lambda_callback, &lambda);
///
/// unsafe {
///     let fun: &unsafe extern "C" fn(u64, u64) -> u64
///         = mem::transmute(closure.code_ptr());
///
///     assert_eq!(11, fun(5, 6));
///     assert_eq!(12, fun(5, 7));
/// }
/// ```
#[derive(Debug)]
pub struct Closure<'a> {
    _cif:    Box<Cif>,
    alloc:   *mut ::low::ffi_closure,
    code:    CodePtr,
    _marker: PhantomData<&'a ()>,
}

impl<'a> Drop for Closure<'a> {
    fn drop(&mut self) {
        unsafe {
            low::closure_free(self.alloc);
        }
    }
}

impl<'a> Closure<'a> {
    /// Creates a new closure with immutable userdata.
    ///
    /// # Arguments
    ///
    /// - `cif` — describes the calling convention and argument and
    ///   result types
    /// - `callback` — the function to call when the closure is invoked
    /// - `userdata` — the pointer to pass to `callback` along with the
    ///   arguments when the closure is called
    ///
    /// # Result
    ///
    /// The new closure.
    pub fn new<U, R>(cif:      Cif,
                     callback: Callback<U, R>,
                     userdata: &'a U) -> Self
    {
        let cif = Box::new(cif);
        let (alloc, code) = low::closure_alloc();

        unsafe {
            low::prep_closure(alloc,
                              cif.as_raw_ptr(),
                              callback,
                              userdata as *const U,
                              code).unwrap();
        }

        Closure {
            _cif:    cif,
            alloc:   alloc,
            code:    code,
            _marker: PhantomData,
        }
    }

    /// Creates a new closure with mutable userdata.
    ///
    /// # Arguments
    ///
    /// - `cif` — describes the calling convention and argument and
    ///   result types
    /// - `callback` — the function to call when the closure is invoked
    /// - `userdata` — the pointer to pass to `callback` along with the
    ///   arguments when the closure is called
    ///
    /// # Result
    ///
    /// The new closure.
    pub fn new_mut<U, R>(cif:      Cif,
                         callback: CallbackMut<U, R>,
                         userdata: &'a mut U) -> Self
    {
        let cif = Box::new(cif);
        let (alloc, code) = low::closure_alloc();

        unsafe {
            low::prep_closure_mut(alloc,
                                  cif.as_raw_ptr(),
                                  callback,
                                  userdata as *mut U,
                                  code).unwrap();
        }

        Closure {
            _cif:    cif,
            alloc:   alloc,
            code:    code,
            _marker: PhantomData,
        }
    }

    /// Obtains the callable code pointer for a closure.
    ///
    /// # Safety
    ///
    /// The result needs to be transmuted to the correct type before
    /// it can be called. If the type is wrong then undefined behavior
    /// will result.
    pub fn code_ptr(&self) -> &unsafe extern "C" fn() {
        self.code.as_fun()
    }
}

/// The type of callback invoked by a
/// [`ClosureOnce`](struct.ClosureOnce.html).
pub type CallbackOnce<U, R> = CallbackMut<Option<U>, R>;

/// A closure that owns needs-drop data.
///
/// This allows the closure’s callback to take ownership of the data, in
/// which case the userdata will be gone if called again.
#[derive(Debug)]
pub struct ClosureOnce(ClosureOwned);

impl ClosureOnce {
    /// Creates a new closure with owned userdata.
    ///
    /// # Arguments
    ///
    /// - `cif` — describes the calling convention and argument and
    ///   result types
    /// - `callback` — the function to call when the closure is invoked
    /// - `userdata` — the value to pass to `callback` along with the
    ///   arguments when the closure is called
    ///
    /// # Result
    ///
    /// The new closure.
    pub fn new<U: Any, R>(cif:      Cif,
                          callback: CallbackOnce<U, R>,
                          userdata: U)
                          -> Self
    {
        ClosureOnce(ClosureOwned::new(cif, callback, Some(userdata)))
    }

    /// Obtains the callable code pointer for a closure.
    ///
    /// # Safety
    ///
    /// The result needs to be transmuted to the correct type before
    /// it can be called. If the type is wrong then undefined behavior
    /// will result.
    pub fn code_ptr(&self) -> &unsafe extern "C" fn() {
        self.0.code_ptr()
    }
}

/// A closure that owns userdata.
#[derive(Debug)]
pub struct ClosureOwned {
    alloc:     *mut ::low::ffi_closure,
    code:      CodePtr,
    _cif:      Box<Cif>,
    _userdata: Box<Any>,
}

impl Drop for ClosureOwned {
    fn drop(&mut self) {
        unsafe {
            low::closure_free(self.alloc);
        }
    }
}

impl ClosureOwned {
    /// Creates a new closure with owned userdata.
    ///
    /// # Arguments
    ///
    /// - `cif` — describes the calling convention and argument and
    ///   result types
    /// - `callback` — the function to call when the closure is invoked
    /// - `userdata` — the value to pass to `callback` along with the
    ///   arguments when the closure is called
    ///
    /// # Result
    ///
    /// The new closure.
    pub fn new<U: Any, R>(cif:      Cif,
                          callback: CallbackMut<U, R>,
                          userdata: U)
                          -> Self
    {
        let cif = Box::new(cif);
        let mut userdata = Box::new(userdata) as Box<Any>;
        let (alloc, code) = low::closure_alloc();

        assert!(!alloc.is_null(), "closure_alloc: returned null");

        unsafe {
            let borrow = userdata.downcast_mut::<U>().unwrap();
            low::prep_closure_mut(alloc,
                                  cif.as_raw_ptr(),
                                  callback,
                                  borrow as *mut U,
                                  code).unwrap();
        }

        ClosureOwned {
            alloc:     alloc,
            code:      code,
            _cif:      cif,
            _userdata: userdata,
        }
    }

    /// Obtains the callable code pointer for a closure.
    ///
    /// # Safety
    ///
    /// The result needs to be transmuted to the correct type before
    /// it can be called. If the type is wrong then undefined behavior
    /// will result.
    pub fn code_ptr(&self) -> &unsafe extern "C" fn() {
        self.code.as_fun()
    }
}

#[cfg(test)]
mod test {
    use low;
    use super::*;
    use std::mem;
    use std::os::raw::c_void;

    #[test]
    fn call() {
        let cif  = Cif::new(vec![Type::i64(), Type::i64()].into_iter(),
                            Type::i64());
        let f    = |m: i64, n: i64| -> i64 {
            unsafe { cif.call(CodePtr(add_it as *mut c_void),
                              &[arg(&m), arg(&n)]) }
        };

        assert_eq!(12, f(5, 7));
        assert_eq!(13, f(6, 7));
        assert_eq!(15, f(8, 7));
    }

    extern "C" fn add_it(n: i64, m: i64) -> i64 {
        return n + m;
    }

    #[test]
    fn closure() {
        let cif  = Cif::new(vec![Type::u64()].into_iter(), Type::u64());
        let env: u64 = 5;
        let closure = Closure::new(cif, callback, &env);

        unsafe {
            let fun: &unsafe extern "C" fn(u64) -> u64
                = mem::transmute(closure.code_ptr());

            assert_eq!(11, fun(6));
            assert_eq!(12, fun(7));
        }
    }

    unsafe extern "C" fn callback(_cif: &low::ffi_cif,
                                  result: &mut u64,
                                  args: *const *const c_void,
                                  userdata: &u64)
    {
        let args: *const &u64 = mem::transmute(args);
        *result = **args + *userdata;
    }

    #[test]
    fn rust_lambda() {
        let cif = Cif::new(vec![Type::u64(), Type::u64()].into_iter(),
                           Type::u64());
        let env = |x: u64, y: u64| x + y;
        let closure = Closure::new(cif, callback2, &env);

        unsafe {
            let fun: &unsafe extern "C" fn (u64, u64) -> u64
                = mem::transmute(closure.code_ptr());

            assert_eq!(11, fun(5, 6));
        }
    }

    unsafe extern "C" fn callback2<F: Fn(u64, u64) -> u64>
        (_cif: &low::ffi_cif,
         result: &mut u64,
         args: *const *const c_void,
         userdata: &F)
    {
        let args: *const &u64 = mem::transmute(args);
        let arg1 = **args.offset(0);
        let arg2 = **args.offset(1);

        *result = userdata(arg1, arg2);
    }

    #[test]
    fn closure_owned() {
        let cif = Cif::new(vec![Type::u64()].into_iter(), Type::u64());
        let mut x = 0u64;
        let userdata = move |y: u64| { x += y; x };
        let closure = ClosureOwned::new(cif, callback3, userdata);

        unsafe {
            let fun: &unsafe extern "C" fn (u64) -> u64
                = mem::transmute(closure.code_ptr());

            assert_eq!(5, fun(5));
            assert_eq!(11, fun(6));
        }
    }

    unsafe extern "C" fn callback3<F: FnMut(u64) -> u64>
        (_cif: &low::ffi_cif,
         result: &mut u64,
         args: *const *const c_void,
         userdata: &mut F)
    {
        let args: *const &u64 = mem::transmute(args);
        let arg1 = **args.offset(0);

        *result = userdata(arg1);
    }
}
