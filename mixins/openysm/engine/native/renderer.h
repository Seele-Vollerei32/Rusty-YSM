#pragma once
#include <cstddef>
#include <cstdint>
struct YsmVertex { float position[3], uv[2], normal[3]; int32_t light; uint32_t hidden; };
extern "C" {
void *ysm_mesh_create(const uint8_t *, size_t) noexcept;
void ysm_mesh_free(void *) noexcept;
size_t ysm_vertex_count(const void *) noexcept;
size_t ysm_bone_count(const void *) noexcept;
const void *ysm_gpu_vertices(const void *) noexcept;
const uint32_t *ysm_gpu_indices(const void *) noexcept;
bool ysm_bones(void *, const float *, size_t, int, void *, size_t) noexcept;
bool ysm_skin(void *, const void *, size_t, const float *, uint32_t, YsmVertex *, size_t) noexcept;
}
