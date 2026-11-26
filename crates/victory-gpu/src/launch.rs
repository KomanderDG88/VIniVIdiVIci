//! Mesh draws launched by the card (`VICTORY_MESH_LAUNCH=1`; off by
//! default): an amplification shader of ours (`launch.hlsl`) in front of
//! every translated mesh shader.
//!
//! The title draws its meshes one indirect record a call, each from the
//! next slot of an argument buffer its own compute passes wrote, and each
//! record is bounded by a compute dispatch of ours between two waits for
//! every earlier shader write (`crate::indirect::bound`): the card goes
//! from drawing to compute and back, emptying itself twice, for every
//! record. (The fork this comes from measured, in the first rooms on an
//! RTX 5080, 3,200 of a frame's 3,950 full waits spent so, and an indirect
//! mesh draw costing twice what the same draw did when the CPU knew its
//! group count.)
//!
//! With the switch the record is read by the draw itself: the pipeline's
//! amplification stage loads its three words where the title's passes
//! wrote them, bounds them as the clamp does (65,535 groups,
//! one-dimensional), charges the list's budget as the clamp does (a record
//! that does not fit runs empty), and launches the mesh groups. The title's
//! own barrier orders its passes before the draw, as on the console;
//! nothing of ours runs between draws. A mesh dispatch the CPU knows
//! (`DispatchMesh`) gives its count as a root constant and is launched the
//! same way, unbounded by the budget as before.
//!
//! The draws' amplification groups may run in another order than the
//! list's (nothing orders them now), so when a list's budget runs out,
//! which of its records run empty is the card's order and no longer the
//! list's: the bound on the list's total work is the same.
//!
//! Only a single record without a count buffer is launched. An indirect
//! mesh call with a count buffer or more than one record keeps the clamp
//! and runs on the same pipeline made without the amplification stage
//! (`pipeline::host::Launch::plain`, made the first time one needs it):
//! the translated mesh shader declares the payload either way
//! (`victory_shader::emit::mesh_payload_bytes`), which the driver takes
//! with or without an amplification shader. So do all indirect mesh calls
//! while `VICTORY_INDIRECT_WINDOW=1` (and with it `VICTORY_INDIRECT_GATHER`)
//! is asked for (`crate::draw`): the windows and gathers clamp mesh records
//! ahead and serve them from a scratch buffer of their own, which the
//! launch would bypass; the two are not mixed.
//!
//! The stage is compiled at run time from the source embedded here, by the
//! Windows SDK's `dxcompiler.dll` ([`dxc`]); without one the switch does
//! nothing but give the mesh shaders their payload (which every mesh
//! pipeline takes without the stage), and every record is clamped as
//! before.
//!
//! Adapted from stellarr-studio/kelvaroq 32c4ab4f,
//! native/patches/0041-mesh-launch-wide-hash.patch (GPL-3.0).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use windows::Win32::Graphics::Direct3D12::*;

use crate::host::HostList;

mod dxc;

/// The amplification stage's source, compiled at run time ([`code`]).
const SOURCE: &str = include_str!("launch.hlsl");

/// The root constant that has the card read the record.
pub(crate) const DIRECT_NONE: u32 = u32::MAX;

/// Where the amplification shader binds: `b0`, `t0`, `u0` in this space.
const SPACE: u32 = victory_shader::emit::LAUNCH_SPACE;

/// Whether the switch is on (`VICTORY_MESH_LAUNCH=1`): the translated mesh
/// shaders declare the payload (the translator reads the switch itself, so
/// the shader cache keys on it), and mesh pipelines get the stage if it
/// compiles ([`ready`]).
pub(crate) fn on() -> bool {
    victory_shader::emit::mesh_launch()
}

/// Whether mesh pipelines get the amplification stage: the switch is on
/// and the stage compiled.
pub(crate) fn ready() -> bool {
    on() && bytes().is_some()
}

/// The stage's code, compiled the first time it is asked for (`None`: the
/// switch is off, or there is no compiler, or it refused the source).
pub(crate) fn bytes() -> Option<&'static [u8]> {
    static CODE: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    if !on() {
        return None;
    }
    CODE.get_or_init(|| match compile_stage() {
        Ok(code) => {
            tracing::info!(
                bytes = code.len(),
                "the mesh launch stage compiled (VICTORY_MESH_LAUNCH=1): single indirect mesh records are launched by the draw"
            );
            Some(code)
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "the mesh launch stage did not compile (VICTORY_MESH_LAUNCH=1): mesh pipelines are made without it, every record clamped as before"
            );
            None
        }
    })
    .as_deref()
}

/// Compiles the stage (whatever the switch says: the test runs it): the
/// container, hashed as D3D12 checks it.
pub(crate) fn compile_stage() -> Result<Vec<u8>, String> {
    let mut code = dxc::compile(SOURCE, "main", "as_6_6")?;
    // Unsigned (no `dxil.dll` beside the compiler): the hash the
    // translations carry, which is all D3D12 checks.
    if code.get(4..20).is_some_and(|h| h.iter().all(|&b| b == 0)) {
        tracing::info!("the mesh launch stage came unsigned from the compiler: hashed as the translations are");
        victory_shader::emit::sign_container(&mut code);
    }
    Ok(code)
}

/// The stage's code for a pipeline stream (empty if there is none).
pub(crate) fn code() -> D3D12_SHADER_BYTECODE {
    bytes().map_or(D3D12_SHADER_BYTECODE::default(), |b| D3D12_SHADER_BYTECODE {
        pShaderBytecode: b.as_ptr().cast(),
        BytecodeLength: b.len(),
    })
}

/// The three root parameters the stage reads, in order: the direct group
/// count (one constant), the record (a root SRV), the list's budget (a
/// root UAV). Five root words.
pub(crate) fn params() -> [D3D12_ROOT_PARAMETER1; 3] {
    let descriptor = |kind| D3D12_ROOT_PARAMETER1 {
        ParameterType: kind,
        Anonymous: D3D12_ROOT_PARAMETER1_0 {
            Descriptor: D3D12_ROOT_DESCRIPTOR1 {
                ShaderRegister: 0,
                RegisterSpace: SPACE,
                Flags: D3D12_ROOT_DESCRIPTOR_FLAG_DATA_VOLATILE,
            },
        },
        ShaderVisibility: D3D12_SHADER_VISIBILITY_AMPLIFICATION,
    };
    [
        D3D12_ROOT_PARAMETER1 {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER1_0 {
                Constants: D3D12_ROOT_CONSTANTS { ShaderRegister: 0, RegisterSpace: SPACE, Num32BitValues: 1 },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_AMPLIFICATION,
        },
        descriptor(D3D12_ROOT_PARAMETER_TYPE_SRV),
        descriptor(D3D12_ROOT_PARAMETER_TYPE_UAV),
    ]
}

/// Mesh draws launched since the last line: records read by the card, and
/// dispatches whose count the CPU gave.
static LAUNCHED: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

/// Records the launch of one mesh draw, the mesh pipeline with its stage
/// bound and the stage's parameters at `roots`: the record at an address
/// on the card (the card reads it), or the group count the CPU knows.
/// False if the list has no budget for a record, or no mesh dispatch.
pub(crate) fn record(list: &HostList, roots: [u32; 3], what: Launch) -> bool {
    let Ok(l6) = windows::core::Interface::cast::<ID3D12GraphicsCommandList6>(&list.list) else {
        return false;
    };
    let budget = list.budget();
    let (direct, record, budget) = match what {
        Launch::Record(address) => {
            let Some(budget) = budget else {
                return false;
            };
            (DIRECT_NONE, address, budget)
        }
        // The stage reads neither the record nor the budget; live
        // addresses all the same where the list has them.
        Launch::Direct(x) => (x, budget.unwrap_or(0), budget.unwrap_or(0)),
    };
    // SAFETY: an open list with the mesh pipeline and its root signature
    // set; the record and the budget are live buffers the list keeps.
    unsafe {
        list.list.SetGraphicsRoot32BitConstant(roots[0], direct, 0);
        list.list.SetGraphicsRootShaderResourceView(roots[1], record);
        list.list.SetGraphicsRootUnorderedAccessView(roots[2], budget);
        l6.DispatchMesh(1, 1, 1);
    }
    LAUNCHED[usize::from(matches!(what, Launch::Direct(_)))].fetch_add(1, Ordering::Relaxed);
    true
}

/// What one launch reads.
#[derive(Clone, Copy)]
pub(crate) enum Launch {
    /// The record at this address on the card.
    Record(u64),
    /// A group count known on the CPU.
    Direct(u32),
}

/// For the frame-rate line.
pub(crate) fn line(frames: f64) {
    let [records, direct] = [0, 1].map(|i| LAUNCHED[i].swap(0, Ordering::Relaxed));
    if records + direct > 0 {
        tracing::info!(
            "mesh draws launched by the card per frame (VICTORY_MESH_LAUNCH=1): {:.0} records read where the title wrote them, \
             {:.0} counts known on the CPU (no pass of ours between them)",
            records as f64 / frames,
            direct as f64 / frames
        );
    }
}
