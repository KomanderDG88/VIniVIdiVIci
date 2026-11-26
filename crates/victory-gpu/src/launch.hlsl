// The launch of one mesh draw by the card (`launch.rs`;
// `VICTORY_MESH_LAUNCH=1`): an amplification shader of ours in front of
// every translated mesh shader. It reads the title's record where the
// title's passes wrote it, bounds it as the clamp of `indirect.rs` bounds a
// mesh record, charges the list's budget the same way, and launches the
// mesh groups. Nothing of ours runs between the draws: no pass, no wait for
// every earlier shader write.
//
// Embedded as text (`include_str!` in `launch.rs`) and compiled when the
// first mesh pipeline wants it, by the Windows SDK's `dxcompiler.dll`
// (`launch/dxc.rs`): the system's compiler (`D3DCompile`), which compiles
// our other shaders, knows no shader model past 5.1, and an amplification
// shader needs 6.5.

// The payload every translated mesh shader declares and never reads with
// the switch (`victory_shader::emit::MESH_LAUNCH_PAYLOAD_BYTES`, 4 bytes).
struct Payload {
    uint unused;
};
groupshared Payload payload;

// `direct`: the group count of a mesh dispatch the CPU knows (the title's
// DispatchMesh), or 0xffffffff for a record the card reads at `record`.
cbuffer Launch : register(b0, space9) {
    uint direct;
};
// The record: three words, x, y, z (the console's mesh dispatches are
// one-dimensional).
ByteAddressBuffer record : register(t0, space9);
// The list's work so far in its first word; the largest group count a
// record asked for and the groups asked in all in the next two (read back
// when the list has run, as the clamp leaves them).
RWByteAddressBuffer budget : register(u0, space9);

// As in the clamp (`indirect.rs`): the list's budget, and what one mesh
// group charges of it.
static const uint BUDGET = 1u << 27;
static const uint MESH_GROUP_COST = 128u;
static const uint DIRECT_NONE = 0xffffffffu;

[numthreads(1, 1, 1)]
void main() {
    uint3 g = uint3(0, 0, 0);
    if (direct != DIRECT_NONE) {
        // Known on the CPU, as before: no budget, the host's limit only.
        g = uint3(min(direct, 65535u), 1u, 1u);
    } else {
        uint3 asked = record.Load3(0);
        g = uint3(min(asked.x, 65535u), min(asked.y, 1u), min(asked.z, 1u));
        uint cost = g.x * g.y * g.z * MESH_GROUP_COST;
        // As the clamp charges one record: a record that does not fit whole
        // runs empty and takes the rest of the budget.
        bool fit = true;
        if (cost > 0) {
            uint before;
            budget.InterlockedOr(0, 0, before);
            fit = false;
            [allow_uav_condition]
            while (before < BUDGET) {
                uint next = (BUDGET - before <= cost) ? BUDGET : (before + cost);
                uint prev;
                budget.InterlockedCompareExchange(0, before, next, prev);
                if (prev == before) {
                    fit = BUDGET - before >= cost;
                    break;
                }
                before = prev;
            }
        }
        if (!fit) {
            g = uint3(0, 0, 0);
        }
        // The clamp's notes, for the frame line and the spread switch.
        uint was;
        budget.InterlockedMax(4, asked.x, was);
        budget.InterlockedAdd(8, min(asked.x, 1u << 24), was);
    }
    payload.unused = 0;
    DispatchMesh(g.x, g.y, g.z, payload);
}
