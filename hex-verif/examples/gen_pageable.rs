// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
// SPDX-License-Identifier: BSD-3-Clause-Clear

//! Generate a pageable test program and write it to stdout.

use hex_instset::database::InstructionDb;
use hex_prog::recipe::ExecutionMode;
use hex_prog::recipe_file::RecipeFile;
use hex_prog::template::ProgramGenerator;
use std::path::Path;

fn main() {
    let db = InstructionDb::load_from_json(Path::new("instructions.json")).unwrap();
    let rf = RecipeFile::load(Path::new("recipes/pageable.toml")).unwrap();
    let mut recipe = rf.into_recipe(42).unwrap();
    recipe.num_packets = 5;
    recipe.execution_mode = ExecutionMode::Direct;

    let gen = ProgramGenerator::new(&db);
    let program = gen.generate(&recipe).unwrap();
    print!("{}", program.assembly);
}
