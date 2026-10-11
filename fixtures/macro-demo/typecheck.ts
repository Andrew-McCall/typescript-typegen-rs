/// <reference lib="es2022" />
/// <reference path="./ambient.d.ts" />
import { createClient, type Sender, type RouteResult } from "./bindings/client";
import type { PatientView } from "./bindings/PatientView";

type Expect<T extends true> = T;
type Equal<A, B> = (<T>() => T extends A ? 1 : 2) extends <T>() => T extends B ? 1 : 2 ? true : false;

(async () => {
  const sender: Sender = { send: async () => ({ ok: true, data: undefined as never, status: 200 }) };
  const api = createClient(sender);

  const res = await api.patient.getPatient({ id: 1 });
  type _Narrowed = Expect<Equal<typeof res, RouteResult<PatientView, unknown>>>;

  if (res.ok) {
    const _data: PatientView = res.data;
  } else {
    const _status: number = res.status;
  }

  // @ts-expect-error reading data without narrowing must not type-check
  res.data;

  // @ts-expect-error a misspelled argument key must not type-check
  await api.patient.getPatient({ idd: 1 });

  // A concrete query type has no index signature; this must still type-check
  // against `RouteRequest.query: Record<string, unknown>`.
  const searched = await api.patient.searchPatients({ query: { name: "a" } });
  type _SearchedNarrowed = Expect<Equal<typeof searched, RouteResult<PatientView[], unknown>>>;

  // `update_patient`'s `Request = "PatientView"` override must be the body type
  // the client actually takes -- not its literal (unexported) wire-form extractor type.
  const updated = await api.patient.updatePatient({ id: 1, body: { id: 1, name: "a" } });
  type _UpdatedNarrowed = Expect<Equal<typeof updated, RouteResult<PatientView, unknown>>>;

  // @ts-expect-error a field outside the override type must not type-check
  await api.patient.updatePatient({ id: 1, body: { id: 1, name: "a", extra: true } });
})();
