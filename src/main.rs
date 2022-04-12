use std::env;
use std::fs::File;
use std::io::Read;
use std::thread;
use std::time::Duration;

use xorshift::{Rng, SeedableRng, Xoroshiro128};

use sdl2::Sdl;
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::pixels::{PixelFormatEnum};
use sdl2::render::{TextureAccess, WindowCanvas};

const FRAMEBUFFER_WIDTH: usize = 64;
const FRAMEBUFFER_HEIGHT: usize = 32;
const FRAMEBUFFER_SIZE: usize = FRAMEBUFFER_WIDTH * FRAMEBUFFER_HEIGHT;
const FRAMEBUFFER_PITCH: usize = FRAMEBUFFER_WIDTH * 4;

const DISPLAY_SCALE: usize = 16;

fn main() {
    let rom_path = env::args().skip(1).next().expect("Missing path argument");

    let mut rom: Vec<u8> = Vec::new();

    let mut rom_file = File::open(rom_path)
        .expect("Failed to open ROM file");
    rom_file
        .read_to_end(&mut rom)
        .expect("Failed to read ROM file");

    let mut app = Application::new(rom);
    app.run();
}

struct Application {
    sdl: Sdl,
    cpu: Chip8,
    canvas: WindowCanvas
}

impl Application {
    pub fn new(rom: Vec<u8>) -> Self {
        let sdl = sdl2::init().expect("Failed to initialize SDL2");
        let video_sys = sdl
            .video()
            .expect("Failed to initialize SDL2 Video");

        let cpu = Chip8::new(&rom)
            .expect("Failed to initialize CHIP-8 CPU");

        let window = video_sys
            .window("chip8-rs",
                    (FRAMEBUFFER_WIDTH * DISPLAY_SCALE) as u32,
                    (FRAMEBUFFER_HEIGHT * DISPLAY_SCALE) as u32)
            .opengl()
            .position_centered()
            .build()
            .expect("Failed to create SDL2 window");

        let canvas = window
            .into_canvas()
            .build()
            .expect("Failed to create SDL2 window surface");

        Application {
            sdl,
            cpu,
            canvas
        }
    }

    pub fn run(&mut self) {
        let mut events = self.sdl
            .event_pump()
            .expect("Failed to create event pump");

        let mut close = false;

        let texture_creator = self.canvas.texture_creator();

        let mut texture = texture_creator
            .create_texture(PixelFormatEnum::RGB888, TextureAccess::Streaming,
                            FRAMEBUFFER_WIDTH as u32, FRAMEBUFFER_HEIGHT as u32)
            .expect("Failed to create streaming texture");

        while !close {
            for event in events.poll_iter() {
                match event {
                    Event::Quit { .. } => close = true,
                    Event::KeyDown { keycode, .. } => {
                        if let Some(key) = keycode {
                            self.cpu.set_key_state(key, true);
                        }
                    }
                    Event::KeyUp { keycode, .. } => {
                        if let Some(key) = keycode {
                            self.cpu.set_key_state(key, false);
                        }
                    }
                    _ => (),
                }
            }

            self.cpu.step();

            texture.update(None, self.cpu.get_framebuffer(), FRAMEBUFFER_PITCH)
                .expect("Failed to update texture");

            self.canvas.copy(&texture, None, None)
                .expect("Failed to copy texture");

            self.canvas.present();

            if cfg!(debug_assertions) {
                print!(
                    "OP:\t{:04X}\t| PC: \t{:04X}\t| I:\t{:04X}\t| SP:\t{:02X}\t",
                    self.cpu.get_opcode(),
                    self.cpu.get_program_counter(),
                    self.cpu.get_program_index(),
                    self.cpu.get_stack_pointer()
                );

                print!("\nS: \t");
                self.cpu
                    .get_stack()
                    .iter()
                    .for_each(|b| print!("{:04X} ", b));

                print!("\nV: \t");
                self.cpu
                    .get_registers()
                    .iter()
                    .for_each(|b| print!("{:02X} ", b));

                print!("\n\n");
            }

            thread::sleep(Duration::from_millis(16));
        }
    }
}

struct Chip8 {
    memory: [u8; 4096],
    registers: [u8; 16],
    framebuffer: [u32; FRAMEBUFFER_SIZE],
    stack: [u16; 16],
    keys: [bool; 16],
    opcode: u16,

    index: u16,
    program_counter: u16,
    stack_pointer: usize,

    random: Xoroshiro128,

    delay_timer: u8,
    sound_timer: u8,

    beep_flag: bool,

    last_key: Option<usize>,
}

impl Chip8 {
    fn new(rom: &[u8]) -> Result<Self, String> {
        let mut chip8 = Chip8 {
            memory: [0; 4096],
            registers: [0; 16],
            framebuffer: [0; FRAMEBUFFER_SIZE],
            stack: [0; 16],
            keys: [false; 16],
            opcode: 0,

            index: 0,
            program_counter: 0,
            stack_pointer: 0,

            random: Xoroshiro128::from_seed(&[0x7020de7ee5e88ab7, 0xe587fbb5ba4fccee]),

            delay_timer: 0,
            sound_timer: 0,

            beep_flag: false,

            last_key: None,
        };

        chip8.load_fontset(include_bytes!("fontset.bin"))?;
        chip8.load_rom(rom)?;

        Ok(chip8)
    }

    fn load_fontset(&mut self, bytes: &[u8]) -> Result<(), String> {
        let start = 0x050;
        let end = 0x0A0;
        let len = end - start;

        if bytes.len() > len {
            Err(format!("Fontset ROM exceeds maximum size (cap: {}, len: {})", len, bytes.len()))
        } else {
            self.memory[start..end]
                .copy_from_slice(bytes);

            Ok(())
        }
    }

    fn load_rom(&mut self, bytes: &[u8]) -> Result<usize, String> {
        let start = 0x200;
        let end = self.memory.len();

        if bytes.len() > end - start {
            Err(format!("Game ROM exceeds maximum size (cap: {}, len: {})", end - start, bytes.len()))
        } else {
            self.program_counter = start as u16;
            self.index = 0x0;

            self.stack = [0u16; 16];
            self.stack_pointer = 0;

            for i in 0..bytes.len() {
                self.memory[i + start] = bytes[i];
            }

            Ok(self.memory.len())
        }
    }

    fn step(&mut self) {
        self.opcode = (self.memory[self.program_counter as usize] as u16) << 8
            | self.memory[self.program_counter as usize + 1] as u16;

        match self.opcode & 0xF000 {
            // 0NNN - Calls RCA 1802 program at address NNN
            0x0000 => {
                match self.opcode & 0x0FFF {
                    0x0000 => {
                        self.program_counter += 2;
                    }
                    // 00E0 - Clear framebuffer
                    0x00E0 => {
                        self.framebuffer.fill(0);
                        self.program_counter += 2;
                    }
                    // 00EE - Returns from subroutine
                    0x00EE => {
                        if self.stack_pointer <= 0 {
                            panic!("Couldn't pop from stack (stack is empty)");
                        }

                        self.stack_pointer -= 1;

                        self.program_counter = self.stack[self.stack_pointer];
                        self.program_counter += 2;
                    }
                    _ => panic!("Unknown instruction ({:04X})", self.opcode),
                }
            }
            // 1NNN - Jumps to address NNN
            0x1000 => {
                self.program_counter = self.opcode & 0x0FFF;
            }
            // 2NNN - Calls subroutine at NNN
            0x2000 => {
                if self.stack_pointer >= 15 {
                    panic!("Couldn't push into stack (stack has exceeded maximum size)");
                }

                self.stack[self.stack_pointer] = self.program_counter;
                self.stack_pointer += 1;

                self.program_counter = self.opcode & 0x0FFF;
            }
            // 3XNN - Skips the next instruction if VX equals NN
            0x3000 => {
                if self.registers[(self.opcode as usize & 0x0F00) >> 8]
                    == (self.opcode & 0x00FF) as u8
                {
                    self.program_counter += 4;
                } else {
                    self.program_counter += 2;
                }
            }
            // 4XNN - Skips the next instruction if VX does not equal NN
            0x4000 => {
                if self.registers[(self.opcode as usize & 0x0F00) >> 8]
                    != (self.opcode & 0x00FF) as u8
                {
                    self.program_counter += 4;
                } else {
                    self.program_counter += 2;
                }
            }
            // 5XY0 - Skips the next instruction if VX equals VY
            0x5000 => {
                if self.registers[(self.opcode as usize & 0x0F00) >> 8]
                    == self.registers[(self.opcode as usize & 0x00F0) >> 4]
                {
                    self.program_counter += 4;
                } else {
                    self.program_counter += 2;
                }
            }
            // 6XNN - Sets VX to NN
            0x6000 => {
                self.registers[(self.opcode as usize & 0x0F00) >> 8] = (self.opcode & 0x00FF) as u8;
                self.program_counter += 2;
            }
            // 7XNN - Adds NN to VX (carry flag is not changed)
            0x7000 => {
                let (result, _) = self.registers[(self.opcode as usize & 0x0F00) >> 8]
                    .overflowing_add((self.opcode & 0x00FF) as u8);

                self.registers[(self.opcode as usize & 0x0F00) >> 8] = result;
                self.program_counter += 2;
            }
            // 8XNO - Sets VX to a value calculated from VX and VY
            0x8000 => {
                let x = (self.opcode as usize & 0x0F00) >> 8;
                let y = (self.opcode as usize & 0x00F0) >> 4;

                match self.opcode & 0x000F {
                    // 8XY0 - Sets VX to VY
                    0x0000 => self.registers[x] = self.registers[y],
                    // 8XY1 - Sets VX to VX OR VY
                    0x0001 => self.registers[x] |= self.registers[y],
                    // 8XY2 - Sets VX to VX AND VY
                    0x0002 => self.registers[x] &= self.registers[y],
                    // 8XY3 - Sets VX to VX XOR VY
                    0x0003 => self.registers[x] ^= self.registers[y],
                    // 8XY4 - Sets VX to VX + VY (sets VF to 1 if a carry occurs, otherwise 0)
                    0x0004 => {
                        let (result, carry) = self.registers[x].overflowing_add(self.registers[y]);

                        self.registers[0xF] = if carry { 1 } else { 0 };
                        self.registers[x] = result;
                    }
                    // 8XY5 - Sets VX to VX - VY (sets VF to 0 if a borrow occurs, otherwise 1)
                    0x0005 => {
                        let (result, borrow) = self.registers[x].overflowing_sub(self.registers[y]);

                        self.registers[0xF] = if borrow { 0 } else { 1 };
                        self.registers[x] = result;
                    }
                    // 8XY6 - Sets VX to VY >> 1 (sets VF to the least significant bit of VY before the shift)
                    0x0006 => {
                        self.registers[0xF] = self.registers[y] & 0b00000001;
                        self.registers[x] = self.registers[y] >> 1;
                    }
                    // 8XY7 - Sets VX to VY - VX. (sets VF to 0 if a borrow occurs, otherwise 1)
                    0x0007 => {
                        let (result, borrow) = self.registers[y].overflowing_sub(self.registers[x]);

                        self.registers[0xF] = if borrow { 0 } else { 1 };
                        self.registers[x] = result;
                    }
                    // 8XYE - Sets VX to VY << 1 (sets VF to the most significant bit of VY before the shift)
                    0x000E => {
                        self.registers[0xF] = self.registers[y] & 0b10000000;
                        self.registers[x] = self.registers[y] << 1;
                    }
                    _ => panic!("Unknown instruction ({:04X})", self.opcode),
                }

                self.program_counter += 2;
            }
            // 9XY0 - Skips the next instruction if VX doesn't equal VY
            0x9000 => {
                if self.registers[(self.opcode as usize & 0x0F00) >> 8]
                    != self.registers[(self.opcode as usize & 0x00F0) >> 4]
                {
                    self.program_counter += 4;
                } else {
                    self.program_counter += 2;
                }
            }
            // ANNN - Sets I to the address NNN
            0xA000 => {
                self.index = self.opcode & 0x0FFF;
                self.program_counter += 2;
            }
            // BNNN - Jumps to the address NNN plus V0
            0xB000 => {
                self.program_counter = (self.opcode & 0x0FFF) + self.registers[0x0] as u16;
            }
            // CXNN - Sets VX to the result of a bitwise and operation on a random number (between 0 and 255) and NN
            0xC000 => {
                self.registers[(self.opcode as usize & 0x0F00) >> 8] =
                    self.rand() & (self.opcode & 0x00FF) as u8;

                self.program_counter += 2;
            }
            // DXYN - Draws a sprite at coordinates (VX, VY) that has the dimensions of 8xN
            0xD000 => {
                let dst_x = self.registers[(self.opcode as usize & 0x0F00) >> 8] as usize;
                let dst_y = self.registers[(self.opcode as usize & 0x00F0) >> 4] as usize;

                let width = 8;
                let height = (self.opcode & 0x000F) as usize;

                self.registers[0xF] = 0;

                for y in 0..height {
                    let src_pixel = self.memory[self.index as usize + y];

                    for x in 0..width {
                        if dst_x + x >= FRAMEBUFFER_WIDTH || dst_y + y >= FRAMEBUFFER_HEIGHT {
                            continue;
                        }

                        if (src_pixel & (0x80 >> x)) != 0 {
                            let dst = (dst_x + x) + ((dst_y + y) * FRAMEBUFFER_WIDTH);

                            if self.framebuffer[dst] != 0 {
                                self.registers[0xF] = 1;
                            }

                            self.framebuffer[dst] ^= 0xFFFFFFFF;
                        }
                    }
                }

                self.program_counter += 2;
            }
            0xE000 => {
                let x = (self.opcode as usize & 0x0F00) >> 8;

                match self.opcode & 0x00FF {
                    // EX9E - Skips the next instruction if the key stored in VX is pressed
                    0x009E => {
                        if self.keys[x] {
                            self.program_counter += 4;
                        } else {
                            self.program_counter += 2;
                        }
                    }
                    // EXA1 - Skips the next instruction if the key stored in VX is not pressed
                    0x00A1 => {
                        if !self.keys[x] {
                            self.program_counter += 4;
                        } else {
                            self.program_counter += 2;
                        }
                    }
                    _ => panic!("Unknown instruction ({:04X})", self.opcode),
                }
            }
            0xF000 => {
                let x = (self.opcode as usize & 0x0F00) >> 8;

                match self.opcode & 0x00FF {
                    // FX07 - Sets VX to the value of the delay timer
                    0x0007 => {
                        self.registers[x] = self.delay_timer;
                        self.program_counter += 2;
                    }
                    // FX0A - Sets VX to the next key press, blocking all other instructions until it is received
                    0x000A => {
                        if let Some(key) = self.last_key {
                            self.registers[x] = key as u8;
                            self.program_counter += 2;
                        }
                    }
                    // FX15 - Sets the delay timer to VX
                    0x0015 => {
                        self.delay_timer = self.registers[x];
                        self.program_counter += 2;
                    }
                    // FX18 - Sets the sound timer to VX
                    0x0018 => {
                        self.sound_timer = self.registers[x];
                        self.program_counter += 2;
                    }
                    // FX1E - Sets I to VX + I
                    0x001E => {
                        self.index += self.registers[x] as u16;
                        self.program_counter += 2;
                    }
                    // FX29 - Sets I to the location of the sprite for the character in VX
                    0x0029 => {
                        let c = self.registers[x] as u16;

                        self.index = 0x050 + (c * 5);
                        self.program_counter += 2;
                    }
                    // FX33 - Sets VX to the binary-coded deciaml representation of I
                    0x0033 => {
                        let x = self.registers[x];

                        self.memory[self.index as usize] = x / 100;
                        self.memory[self.index as usize + 1] = (x / 10) % 10;
                        self.memory[self.index as usize + 2] = (x % 100) % 10;

                        self.program_counter += 2;
                    }
                    // FX55 - Stores V0 to VX (including VX) in memory starting at address I
                    0x0055 => {
                        for x in 0..=x {
                            self.memory[self.index as usize] = self.registers[x];
                            self.index += 1;
                        }

                        self.program_counter += 2;
                    }
                    // FX65 - Fills V0 to VX (including VX) with values from memory starting at address I
                    0x0065 => {
                        for x in 0..=x {
                            self.registers[x] = self.memory[self.index as usize];
                            self.index += 1;
                        }

                        self.program_counter += 2;
                    }
                    _ => panic!("Unknown instruction ({:04X})", self.opcode),
                }
            }
            _ => panic!("Unknown instruction ({:04X})", self.opcode),
        }

        if self.delay_timer > 0 {
            self.delay_timer -= 1;
        }

        if self.sound_timer > 0 {
            if self.sound_timer == 1 {
                self.beep_flag = true;
            }

            self.sound_timer -= 1;
        }

        self.last_key = None;
    }

    pub fn get_registers(&self) -> &[u8; 16] {
        &self.registers
    }

    pub fn get_framebuffer(&self) -> &[u8] {
        let len = self.framebuffer.len();
        let ptr = self.framebuffer.as_ptr() as *const u8;

        unsafe {
            std::slice::from_raw_parts(ptr, len * 4)
        }
    }

    pub fn get_stack(&self) -> &[u16; 16] {
        &self.stack
    }

    pub fn get_opcode(&self) -> u16 {
        self.opcode
    }

    pub fn get_program_counter(&self) -> u16 {
        self.program_counter
    }

    pub fn get_program_index(&self) -> u16 {
        self.index
    }

    pub fn get_stack_pointer(&self) -> usize {
        self.stack_pointer
    }

    pub fn set_key_state(&mut self, key: Keycode, pressed: bool) {
        let i = match key {
            Keycode::Num1 => 0x1,
            Keycode::Num2 => 0x2,
            Keycode::Num3 => 0x3,
            Keycode::Num4 => 0xC,
            Keycode::Q => 0x4,
            Keycode::W => 0x5,
            Keycode::E => 0x6,
            Keycode::R => 0xD,
            Keycode::A => 0x7,
            Keycode::S => 0x8,
            Keycode::D => 0x9,
            Keycode::F => 0xE,
            Keycode::Z => 0xA,
            Keycode::X => 0x0,
            Keycode::C => 0xB,
            Keycode::V => 0xF,
            _ => return,
        };

        self.keys[i] = pressed;
        self.last_key = Some(i);
    }

    fn rand(&mut self) -> u8 {
        (self.random.next_u32() & 0x000000FF) as u8
    }
}
