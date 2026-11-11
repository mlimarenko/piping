import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { Link, MemoryRouter, Route, Routes } from "react-router";
import { expect, it, vi } from "vitest";
import "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";
import ProfileDetail from "./ProfileDetail";
import ProfileAssignmentDetail from "./ProfileAssignmentDetail";
import { PROFILES_KEY } from "./keys";
import type * as Api from "./api";

const api=vi.hoisted(()=>({getProfile:vi.fn(),listAllProfileAssignments:vi.fn(),deleteProfileAssignment:vi.fn()}));
vi.mock("./api",async original=>({...await original<typeof Api>(),...api}));
vi.mock("./useScopeEntities",()=>({useScopeEntities:()=>({optionsByScope:{},resolveName:()=>"Synthetic host",isPending:false})}));

it("refreshes a fresh cached profile after deleting its last assignment and immediately returning",async()=>{
  const now="2026-01-01T00:00:00Z";
  const profile:Api.MonitoringProfile={id:"profile-1",name:"Synthetic profile",slug:"synthetic-profile",enabled:true,items:[],item_count:0,assignment_count:1,created_at:now,updated_at:now};
  const assignment:Api.ProfileAssignment={id:"assignment-1",profile_id:profile.id,profile_name:profile.name,profile_slug:profile.slug,scope_type:"connection_target",scope_id:"host-1",enabled:true,weight:0,created_at:now,updated_at:now};
  let removed=false;
  api.getProfile.mockImplementation(()=>Promise.resolve({...profile,assignment_count:removed?0:1}));
  api.listAllProfileAssignments.mockImplementation(()=>Promise.resolve(removed?[]:[assignment]));
  api.deleteProfileAssignment.mockImplementation(()=>{removed=true;return Promise.resolve();});
  const client=new QueryClient({defaultOptions:{queries:{retry:false,staleTime:30_000,refetchOnWindowFocus:false},mutations:{retry:false}}});
  client.setQueryData([...PROFILES_KEY,profile.id],profile);
  render(<QueryClientProvider client={client}><AllPermissions><MemoryRouter initialEntries={["/observability/profiles/profile-1"]}><Routes>
    <Route path="/observability/profiles/:id" element={<ProfileDetail/>}/>
    <Route path="/observability/profile-assignments/:id" element={<ProfileAssignmentDetail/>}/>
    <Route path="/observability" element={<Link to="/observability/profiles/profile-1">Return to profile</Link>}/>
  </Routes></MemoryRouter></AllPermissions></QueryClientProvider>);
  expect(await screen.findByRole("button",{name:/^delete$/i})).toBeDisabled();
  fireEvent.click(await screen.findByRole("link",{name:/Synthetic host/}));
  fireEvent.click(await screen.findByRole("button",{name:/^delete$/i}));
  fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button",{name:/^delete$/i}));
  fireEvent.click(await screen.findByRole("link",{name:"Return to profile"}));
  await waitFor(()=>expect(screen.getByRole("button",{name:/^delete$/i})).toBeEnabled());
  expect(client.getQueryData<Api.MonitoringProfile>([...PROFILES_KEY,profile.id])?.assignment_count).toBe(0);
  expect(api.getProfile).toHaveBeenCalledTimes(1);
});
