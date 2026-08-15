// Adversarial case 17: a trait with bodyless method signatures (must emit
// nothing) alongside one default-bodied method that does contain a .clone().

trait MyTrait {
    fn bodyless_method(&self) -> i32;

    fn default_method(&self) -> String {
        let s = self.bodyless_method().to_string();
        s.clone()
    }
}

struct Impl;

impl MyTrait for Impl {
    fn bodyless_method(&self) -> i32 {
        0
    }
}
