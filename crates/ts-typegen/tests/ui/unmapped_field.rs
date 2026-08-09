use ts_typegen::Ts;

struct Foreign;

#[derive(Ts)]
struct Bad {
    v: Foreign,
}

fn main() {}
