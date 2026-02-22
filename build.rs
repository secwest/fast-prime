fn main() {
    // Link primesieve static library for V7's fast B sieve
    println!("cargo:rustc-link-search=native=C:/Users/dr/Documents/primesieve/build");
    println!("cargo:rustc-link-lib=static=primesieve");
}
