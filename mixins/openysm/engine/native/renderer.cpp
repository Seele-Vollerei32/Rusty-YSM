// Adapted from OpenYSM.cpp (MIT). JNI/OpenGL are replaced by a bounded C ABI.
#include "renderer.h"
#include <vector>
#include <cmath>
#include <cstring>
#include <functional>
#include <algorithm>
#include <memory>
#include <stdexcept>
#if defined(__aarch64__) || defined(_M_ARM64)
#define SSE2NEON_SUPPRESS_WARNINGS
#include "sse2neon.h"
#else
#include <immintrin.h>
#endif
#if defined(__GLIBC__) || defined(__BIONIC__)
#define FAST_SINCOS(x, s, c) sincosf((x), (s), (c))
#else
inline void ms_sincosf(float x, float *s, float *c) {
    *s = std::sin(x);
    *c = std::cos(x);
}

#define FAST_SINCOS(x, s, c) ms_sincosf((x), (s), (c))
#endif

#if defined(__FMA__)
#define MADD_PS(a, b, c) _mm_fmadd_ps((a), (b), (c))
#else
#define MADD_PS(a, b, c) _mm_add_ps(_mm_mul_ps((a), (b)), (c))
#endif



struct FastQuad {
    int boneIdx;
    bool cullable;
    __m128 x, y, z;
    __m128 u, v;
    float nx, ny, nz;
};

struct NativeBone {
    int parentIdx;
    int partMask;
    bool glow;
    float pivotX, pivotY, pivotZ;
    int quadStart;
    int quadCount;
    int subtreeCount;
    float aabbMin[3];
    float aabbMax[3];
};

struct alignas(16) PrecomputedBoneMats {
    alignas(16) float gb[16];
    __m128 gn_c0, gn_c1, gn_c2;
    int currentLight;
};

enum Uninitialized { UNINITIALIZED };

struct alignas(16) Mat4 {
    float m[16];

    Mat4() { identity(); }

    Mat4(Uninitialized) {
    }

    Mat4(const float *data) { std::memcpy(m, data, 16 * sizeof(float)); }

    inline void identity() {
        std::memset(m, 0, 16 * sizeof(float));
        m[0] = m[5] = m[10] = m[15] = 1.0f;
    }

    inline void mul(const Mat4 &right) {
        __m128 l0 = _mm_load_ps(&m[0]);
        __m128 l1 = _mm_load_ps(&m[4]);
        __m128 l2 = _mm_load_ps(&m[8]);
        __m128 l3 = _mm_load_ps(&m[12]);

        auto mac = [&](const float *r_col) {
            __m128 v = _mm_load_ps(r_col);
            __m128 res = _mm_mul_ps(l0, _mm_shuffle_ps(v, v, _MM_SHUFFLE(0, 0, 0, 0)));
            res = MADD_PS(l1, _mm_shuffle_ps(v, v, _MM_SHUFFLE(1, 1, 1, 1)), res);
            res = MADD_PS(l2, _mm_shuffle_ps(v, v, _MM_SHUFFLE(2, 2, 2, 2)), res);
            res = MADD_PS(l3, _mm_shuffle_ps(v, v, _MM_SHUFFLE(3, 3, 3, 3)), res);
            return res;
        };

        __m128 r0 = mac(&right.m[0]);
        __m128 r1 = mac(&right.m[4]);
        __m128 r2 = mac(&right.m[8]);
        __m128 r3 = mac(&right.m[12]);

        _mm_store_ps(&m[0], r0);
        _mm_store_ps(&m[4], r1);
        _mm_store_ps(&m[8], r2);
        _mm_store_ps(&m[12], r3);
    }

    inline Mat4 normalMatrix4x4() const {
        Mat4 res(UNINITIALIZED);
        res.m[0] = m[0];
        res.m[1] = m[1];
        res.m[2] = m[2];
        res.m[3] = 0.0f;
        res.m[4] = m[4];
        res.m[5] = m[5];
        res.m[6] = m[6];
        res.m[7] = 0.0f;
        res.m[8] = m[8];
        res.m[9] = m[9];
        res.m[10] = m[10];
        res.m[11] = 0.0f;
        res.m[12] = 0.0f;
        res.m[13] = 0.0f;
        res.m[14] = 0.0f;
        res.m[15] = 1.0f;
        return res;
    }
};

struct NativeModel {
    std::vector<NativeBone> bones;
    std::vector<FastQuad> fastQuads;
    std::vector<int> evalOrder;

    std::vector<Mat4> cacheGlobalTransforms;
    std::vector<Mat4> cacheGlobalNormals;
    std::vector<PrecomputedBoneMats> cachePrecompMats;
    std::vector<int> visibleBones;
};

struct GpuVertex {
    float pos[3];
    float uv[2];
    uint32_t normal;
    uint16_t boneId;
    uint8_t partMask;
    uint8_t flags;
    uint32_t pad;
};

static_assert(sizeof(GpuVertex) == 32, "GpuVertex size error");

struct BoneDataOut {
    float transform[16];
    float normal[16];
    int32_t packedLight;
    int32_t isHidden;
    int32_t pad[2];
};

static_assert(sizeof(BoneDataOut) == 144, "BoneDataOut mismatch");

struct NativeGpuMesh {
    std::vector<NativeBone> bones;
    std::vector<int> evalOrder;
    int boneCount = 0;
    std::unique_ptr<GpuVertex[]> vertexData;
    std::unique_ptr<uint32_t[]> indexData;
    int vertexCount = 0;
    int indexCount = 0;
    int partMask1Start = 0, partMask1Count = 0;
    int partMask2Start = 0, partMask2Count = 0;
    int partMask3Start = 0, partMask3Count = 0;
    std::vector<Mat4> globalTransforms;
    std::vector<Mat4> globalNormals;
    std::vector<uint8_t> hiddenInherited;
};

static inline uint32_t packNormal_2_10_10_10_REV(float x, float y, float z) {
    auto pack = [](float v) -> uint32_t {
        int i = static_cast<int>(std::lround(v * 511.0f));
        if (i < -512) i = -512;
        if (i > 511) i = 511;
        return static_cast<uint32_t>(i & 0x3FF);
    };
    return pack(x) | (pack(y) << 10) | (pack(z) << 20);
}


static NativeGpuMesh *build_mesh(const uint8_t *data, size_t length) {
    size_t offset = 0;
    auto readInt = [&]() {
        if (offset + 4 > length) throw std::runtime_error("truncated model");
        int v;
        std::memcpy(&v, data + offset, 4);
        offset += 4;
        return v;
    };
    auto readFloat = [&]() {
        if (offset + 4 > length) throw std::runtime_error("truncated model");
        float v;
        std::memcpy(&v, data + offset, 4);
        offset += 4;
        return v;
    };
    auto readByte = [&]() {
        if (offset >= length) throw std::runtime_error("truncated model");
        uint8_t v = data[offset];
        offset += 1;
        return v;
    };

    auto owner = std::make_unique<NativeGpuMesh>();
    auto *mesh = owner.get();
    int boneCount = readInt();
    if (boneCount < 0 || boneCount > 65535) throw std::runtime_error("bone limit");
    mesh->boneCount = boneCount;
    mesh->bones.resize(boneCount);
    mesh->globalTransforms.resize(boneCount);
    mesh->globalNormals.resize(boneCount);
    mesh->hiddenInherited.assign(boneCount, 0);

    std::vector<std::vector<int> > children(boneCount);

    struct QuadRecord {
        uint32_t vertexOffset;
        uint8_t partMask;
    };
    std::vector<QuadRecord> quadRecords;
    quadRecords.reserve(1024);

    std::vector<GpuVertex> tmpVerts;
    tmpVerts.reserve(4096);

    for (int i = 0; i < boneCount; ++i) {
        NativeBone &bone = mesh->bones[i];
        bone.parentIdx = readInt();
        if (bone.parentIdx < -1 || bone.parentIdx >= i) throw std::runtime_error("parents must precede children");
        if (bone.parentIdx != -1) children[bone.parentIdx].push_back(i);
        bone.partMask = readInt();
        bone.glow = readByte() != 0;
        bone.pivotX = readFloat();
        bone.pivotY = readFloat();
        bone.pivotZ = readFloat();
        bone.quadStart = 0;
        bone.quadCount = 0;
        bone.aabbMin[0] = bone.aabbMin[1] = bone.aabbMin[2] = 0.0f;
        bone.aabbMax[0] = bone.aabbMax[1] = bone.aabbMax[2] = 0.0f;

        int cubeCount = readInt();
        if (cubeCount < 0 || cubeCount > 1000000) throw std::runtime_error("cube limit");
        for (int c = 0; c < cubeCount; ++c) {
            uint8_t cullable = readByte() != 0 ? 1 : 0;
            int qc = readInt();
            if (qc < 0 || qc > 1000000 || size_t(qc) > (length - offset) / 92) throw std::runtime_error("quad limit");
            for (int q = 0; q < qc; ++q) {
                uint32_t vOff = static_cast<uint32_t>(tmpVerts.size());

                float vx[4], vy[4], vz[4], uu[4], vv[4];
                for (int v = 0; v < 4; ++v) {
                    vx[v] = readFloat();
                    vy[v] = readFloat();
                    vz[v] = readFloat();
                }
                for (int v = 0; v < 4; ++v) {
                    uu[v] = readFloat();
                    vv[v] = readFloat();
                }
                float nx = readFloat(), ny = readFloat(), nz = readFloat();
                uint32_t packedNorm = packNormal_2_10_10_10_REV(nx, ny, nz);

                for (int v = 0; v < 4; ++v) {
                    GpuVertex gv;
                    gv.pos[0] = vx[v];
                    gv.pos[1] = vy[v];
                    gv.pos[2] = vz[v];
                    gv.uv[0] = uu[v];
                    gv.uv[1] = vv[v];
                    gv.normal = packedNorm;
                    gv.boneId = static_cast<uint16_t>(i);
                    gv.partMask = static_cast<uint8_t>(bone.partMask);
                    gv.flags = cullable;
                    gv.pad = 0;
                    tmpVerts.push_back(gv);
                }
                quadRecords.push_back({vOff, static_cast<uint8_t>(bone.partMask)});
            }
        }
    }

    // Input is topologically ordered; avoid recursion for deep imported skeletons.
    for (int i = 0; i < boneCount; ++i) {
        mesh->evalOrder.push_back(i);
        mesh->bones[i].subtreeCount = 0;
    }
    for (int i = boneCount - 1; i >= 0; --i) {
        int parent = mesh->bones[i].parentIdx;
        if (parent >= 0) mesh->bones[parent].subtreeCount += mesh->bones[i].subtreeCount + 1;
    }

    std::stable_sort(quadRecords.begin(), quadRecords.end(),
                     [](const QuadRecord &a, const QuadRecord &b) { return a.partMask < b.partMask; });

    mesh->vertexCount = static_cast<int>(tmpVerts.size());
    mesh->vertexData.reset(new GpuVertex[mesh->vertexCount]);
    std::memcpy(mesh->vertexData.get(), tmpVerts.data(), static_cast<size_t>(mesh->vertexCount) * sizeof(GpuVertex));

    mesh->indexCount = static_cast<int>(quadRecords.size()) * 6;
    mesh->indexData.reset(new uint32_t[mesh->indexCount]);

    int currentPartMask = -1;
    int rangeStart = 0;
    auto closeRange = [&](int endIdx) {
        if (currentPartMask < 0) return;
        int count = endIdx - rangeStart;
        switch (currentPartMask) {
            case 1: mesh->partMask1Start = rangeStart;
                mesh->partMask1Count = count;
                break;
            case 2: mesh->partMask2Start = rangeStart;
                mesh->partMask2Count = count;
                break;
            case 3: mesh->partMask3Start = rangeStart;
                mesh->partMask3Count = count;
                break;
            default: break;
        }
    };

    int idxOffset = 0;
    for (const QuadRecord &rec: quadRecords) {
        if (static_cast<int>(rec.partMask) != currentPartMask) {
            closeRange(idxOffset);
            currentPartMask = rec.partMask;
            rangeStart = idxOffset;
        }
        uint32_t v = rec.vertexOffset;
        mesh->indexData[idxOffset + 0] = v + 0;
        mesh->indexData[idxOffset + 1] = v + 1;
        mesh->indexData[idxOffset + 2] = v + 2;
        mesh->indexData[idxOffset + 3] = v + 0;
        mesh->indexData[idxOffset + 4] = v + 2;
        mesh->indexData[idxOffset + 5] = v + 3;
        idxOffset += 6;
    }
    closeRange(idxOffset);

    if (offset != length) throw std::runtime_error("model trailing bytes");
    return owner.release();
}

extern "C" void *ysm_mesh_create(const uint8_t *data, size_t length) noexcept {
    try { return build_mesh(data, length); } catch (...) { return nullptr; }
}
extern "C" void ysm_mesh_free(void *handle) noexcept { delete static_cast<NativeGpuMesh *>(handle); }
extern "C" size_t ysm_vertex_count(const void *handle) noexcept {
    return static_cast<const NativeGpuMesh *>(handle)->vertexCount;
}
extern "C" size_t ysm_bone_count(const void *handle) noexcept {
    return static_cast<const NativeGpuMesh *>(handle)->boneCount;
}
extern "C" const void *ysm_gpu_vertices(const void *handle) noexcept {
    return static_cast<const NativeGpuMesh *>(handle)->vertexData.get();
}
extern "C" const uint32_t *ysm_gpu_indices(const void *handle) noexcept {
    return static_cast<const NativeGpuMesh *>(handle)->indexData.get();
}
extern "C" bool ysm_bones(void *handle, const float *anim, size_t count, int packedLight,
                          void *output, size_t outputLength) noexcept {
    auto *mesh = static_cast<NativeGpuMesh *>(handle);
    if (!mesh || !anim || !output || count != size_t(mesh->boneCount) * 12 ||
        outputLength != size_t(mesh->boneCount) * sizeof(BoneDataOut)) return false;
    auto *outRaw = static_cast<BoneDataOut *>(output);
    const int glowLight = (15 << 4) | (15 << 20);
    std::fill(mesh->hiddenInherited.begin(), mesh->hiddenInherited.end(), 0);

    for (int bIdx: mesh->evalOrder) {
        const NativeBone &bone = mesh->bones[bIdx];

        int pOffset = bIdx * 12;
        float animRx = anim[pOffset + 0], animRy = anim[pOffset + 1], animRz = anim[pOffset + 2];
        float animTx = anim[pOffset + 3], animTy = anim[pOffset + 4], animTz = anim[pOffset + 5];
        float animSx = anim[pOffset + 6], animSy = anim[pOffset + 7], animSz = anim[pOffset + 8];
        float skipChildrenFlag = anim[pOffset + 10];

        float px = bone.pivotX * 0.0625f, py = bone.pivotY * 0.0625f, pz = bone.pivotZ * 0.0625f;
        float dx = px - animTx * 0.0625f;
        float dy = py + animTy * 0.0625f;
        float dz = pz + animTz * 0.0625f;

        float cx, sx, cy, sy, cz, sz;
        FAST_SINCOS(animRx, &sx, &cx);
        FAST_SINCOS(animRy, &sy, &cy);
        FAST_SINCOS(animRz, &sz, &cz);

        Mat4 localMat(UNINITIALIZED);
        localMat.m[0] = (cz * cy) * animSx;
        localMat.m[1] = (sz * cy) * animSx;
        localMat.m[2] = (-sy) * animSx;
        localMat.m[3] = 0.0f;
        localMat.m[4] = (cz * sy * sx - sz * cx) * animSy;
        localMat.m[5] = (sz * sy * sx + cz * cx) * animSy;
        localMat.m[6] = (cy * sx) * animSy;
        localMat.m[7] = 0.0f;
        localMat.m[8] = (cz * sy * cx + sz * sx) * animSz;
        localMat.m[9] = (sz * sy * cx - cz * sx) * animSz;
        localMat.m[10] = (cy * cx) * animSz;
        localMat.m[11] = 0.0f;
        localMat.m[12] = dx - (localMat.m[0] * px + localMat.m[4] * py + localMat.m[8] * pz);
        localMat.m[13] = dy - (localMat.m[1] * px + localMat.m[5] * py + localMat.m[9] * pz);
        localMat.m[14] = dz - (localMat.m[2] * px + localMat.m[6] * py + localMat.m[10] * pz);
        localMat.m[15] = 1.0f;

        bool inheritedHidden = (bone.parentIdx != -1) && mesh->hiddenInherited[bone.parentIdx] != 0;
        bool selfHidden = inheritedHidden || (animSx == 0.0f || animSy == 0.0f || animSz == 0.0f);

        Mat4 &globalMat = mesh->globalTransforms[bIdx];
        Mat4 localNormalMat = localMat.normalMatrix4x4();
        Mat4 &globalNormalMat = mesh->globalNormals[bIdx];

        if (bone.parentIdx != -1) {
            globalMat = mesh->globalTransforms[bone.parentIdx];
            globalMat.mul(localMat);

            globalNormalMat = mesh->globalNormals[bone.parentIdx];
            globalNormalMat.mul(localNormalMat);
        } else {
            globalMat = localMat;
            globalNormalMat = localNormalMat;
        }

        BoneDataOut &out = outRaw[bIdx];
        std::memcpy(out.transform, globalMat.m, 64);
        std::memcpy(out.normal, globalNormalMat.m, 64);
        out.packedLight = bone.glow ? glowLight : packedLight;
        out.isHidden = selfHidden ? 1 : 0;
        out.pad[0] = out.pad[1] = 0;

        mesh->hiddenInherited[bIdx] = (selfHidden || skipChildrenFlag != 0.0f) ? 1 : 0;
    }

    return true;
}
// Four vertices per quad, SIMD transformation using the original Mat4 implementation.
extern "C" bool ysm_skin(void *handle, const void *boneData, size_t boneBytes,
                         const float *root, uint32_t partMask, YsmVertex *out, size_t count) noexcept {
    auto *mesh = static_cast<NativeGpuMesh *>(handle);
    if (!mesh || !boneData || !root || !out || count != size_t(mesh->vertexCount) ||
        boneBytes != size_t(mesh->boneCount) * sizeof(BoneDataOut)) return false;
    const auto *bones = static_cast<const BoneDataOut *>(boneData);
    for (int i = 0; i < mesh->vertexCount; ++i) {
        const auto &v = mesh->vertexData[i];
        const auto &bone = bones[v.boneId];
        Mat4 m(root); m.mul(Mat4(bone.transform));
        __m128 p = _mm_load_ps(m.m + 12);
        p = MADD_PS(_mm_load_ps(m.m), _mm_set1_ps(v.pos[0]), p);
        p = MADD_PS(_mm_load_ps(m.m + 4), _mm_set1_ps(v.pos[1]), p);
        p = MADD_PS(_mm_load_ps(m.m + 8), _mm_set1_ps(v.pos[2]), p);
        alignas(16) float xyz[4]; _mm_store_ps(xyz, p);
        std::copy(xyz, xyz + 3, out[i].position);
        std::copy(v.uv, v.uv + 2, out[i].uv);
        float n[3];
        for (int j = 0; j < 3; ++j) {
            int x = (v.normal >> (j * 10)) & 1023;
            n[j] = float(x >= 512 ? x - 1024 : x) / 511.0f;
        }
        // Inverse transpose of root * bone transform, including nonuniform scale.
        const auto &a = m.m;
        float c0[3] = {a[5]*a[10]-a[6]*a[9], a[6]*a[8]-a[4]*a[10], a[4]*a[9]-a[5]*a[8]};
        float c1[3] = {a[9]*a[2]-a[10]*a[1], a[10]*a[0]-a[8]*a[2], a[8]*a[1]-a[9]*a[0]};
        float c2[3] = {a[1]*a[6]-a[2]*a[5], a[2]*a[4]-a[0]*a[6], a[0]*a[5]-a[1]*a[4]};
        float det = a[0]*c0[0]+a[1]*c0[1]+a[2]*c0[2];
        float length = 0;
        for (int j=0;j<3;++j) {
            out[i].normal[j] = (n[0]*c0[j]+n[1]*c1[j]+n[2]*c2[j]) * (det < 0 ? -1.f : 1.f);
            length += out[i].normal[j]*out[i].normal[j];
        }
        length = std::sqrt(std::max(length, 1e-16f));
        for (float &component : out[i].normal) component /= length;
        out[i].light = bone.packedLight;
        out[i].hidden = bone.isHidden || (partMask && !(v.partMask & partMask));
    }
    return true;
}
