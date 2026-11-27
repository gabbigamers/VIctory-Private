// Reference shader written here (no external source): inline RayQuery in a compute shader.
struct Ray { float3 o; float tmin; float3 d; float tmax; uint mask; uint flags; uint r0; uint r1; };
struct Hit { float t; float u; float v; uint kind; uint inst_index; uint inst_id; uint geom; uint prim; };
RaytracingAccelerationStructure tlas : register(t0);
StructuredBuffer<Ray> rays : register(t1);
RWStructuredBuffer<Hit> hits : register(u0);
[numthreads(64,1,1)]
void main(uint3 id : SV_DispatchThreadID) {
  Ray r = rays[id.x];
  RayDesc rd; rd.Origin = r.o; rd.TMin = r.tmin; rd.Direction = r.d; rd.TMax = r.tmax;
  RayQuery<RAY_FLAG_NONE> q;
  q.TraceRayInline(tlas, r.flags, r.mask, rd);
  while (q.Proceed()) {
    if (q.CandidateType() == CANDIDATE_NON_OPAQUE_TRIANGLE) {
      float2 b = q.CandidateTriangleBarycentrics();
      uint c = (uint)floor(b.x * 8) + (uint)floor(b.y * 8) + q.CandidatePrimitiveIndex();
      if ((c & 1) == 0) q.CommitNonOpaqueTriangleHit();
    }
  }
  Hit h = (Hit)0;
  if (q.CommittedStatus() == COMMITTED_TRIANGLE_HIT) {
    float2 b = q.CommittedTriangleBarycentrics();
    h.t = q.CommittedRayT(); h.u = b.x; h.v = b.y;
    h.kind = q.CommittedTriangleFrontFace() ? 1 : 2;
    h.inst_index = q.CommittedInstanceIndex(); h.inst_id = q.CommittedInstanceID();
    h.geom = q.CommittedGeometryIndex(); h.prim = q.CommittedPrimitiveIndex();
  }
  hits[id.x] = h;
}
