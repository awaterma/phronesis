// D21: Function-ID divergence guard test.
// Every .clone() below lives in a different kind of function context.
// The function IDs must exactly match the IDs the graph emits for defines_fn.

struct Foo;

// We need a generic type for the generic impl fixture
struct GenericFoo<T>(std::marker::PhantomData<T>);

// 1. Method in a generic impl — impl Foo<Bar>
impl GenericFoo<i32> {
    fn generic_impl_method(&self) -> i32 {
        let _x = 42i32.clone();
        0
    }
}

// 2. Method in a plain impl
impl Foo {
    fn plain_impl_method(&self) {
        let _x = 42i32.clone();
    }
}

// 3. Method in a trait impl
trait SomeTrait {
    fn trait_impl_method(&self) -> i32;
}

impl SomeTrait for Foo {
    fn trait_impl_method(&self) -> i32 {
        let _x = 42i32.clone();
        0
    }
}

// 4. Default-bodied trait method
trait TraitWithDefault {
    fn default_method(&self) {
        let _x = 42i32.clone();
    }
}

// 5. Function nested inside a mod block, two levels deep
mod a {
    mod b {
        pub fn deeply_nested() {
            let _x = 42i32.clone();
        }
    }
}

// 6. Plain free function at file top level
fn top_level_function() {
    let _x = 42i32.clone();
}
