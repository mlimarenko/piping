import { useState } from "react";
import { fireEvent, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "@/test/pageChrome";
import { AllPermissions } from "@/test/permissions";
import i18n from "@/shared/i18n";
import EntityFormBody from "@/shared/forms/EntityFormBody";
import { buildEntityDraft, draftToResourceUpdate, objectRefTypeKey, validateEntityDraft } from "@/shared/forms/entityForm";
import { createResource, getResource, listResourcesForTable, updateResource, deleteResource } from "./api";

const api=vi.hoisted(()=>({GET:vi.fn(),POST:vi.fn(),PATCH:vi.fn(),DELETE:vi.fn()}));
vi.mock("@/shared/api/client",()=>({apiClient:api,unwrap:(result:{data?:unknown;error?:unknown})=>{if(result.error)throw result.error instanceof Error?result.error:new Error("API error");return result.data;}}));
const id="00000000-0000-4000-8000-000000000091";
const memberId="00000000-0000-4000-8000-000000000092";
const groupId="00000000-0000-4000-8000-000000000093";
const tagId="00000000-0000-4000-8000-000000000094";
const wire={id,group_id:groupId,group_name:"WAN",member:{object_type:"circuits.circuit",object_id:memberId,label:"Physical"},priority:"primary",tags:[tagId],custom_fields:{},created:null,last_updated:null,piping_synced_at:"2020-02-03T04:05:06Z"};
beforeEach(async()=>{vi.clearAllMocks();await i18n.changeLanguage("en");});

describe("Circuit group assignment native ownership",()=>{
  it("uses native collection/detail and group/member-type/priority filters",async()=>{
    api.GET.mockResolvedValueOnce({data:{items:[wire],total:1,limit:25,offset:0}});
    const page=await listResourcesForTable("Circuit Group Assignments",{limit:25,offset:0,filters:{group_id:[groupId],member_type:["circuits.circuit"],priority:["primary"]}});
    expect(page.items[0]).toMatchObject({name:"WAN · Physical",groupId,member:{objectType:"circuits.circuit",objectId:memberId,label:"Physical"},tags:[tagId],createdAt:undefined});
    expect(api.GET).toHaveBeenCalledWith("/api/v1/inventory/circuit-group-assignments",{params:{query:expect.objectContaining({group_id:groupId,member_type:"circuits.circuit",priority:"primary"})}});
    api.GET.mockResolvedValueOnce({data:wire});
    expect(await getResource("Circuit Group Assignments",id)).toMatchObject({id,groupId});
    expect(api.GET).toHaveBeenLastCalledWith("/api/v1/inventory/circuit-group-assignments/{id}",{params:{path:{id}}});
  });
  it("writes the typed GFK and changed-only metadata, clears nullable priority and membership sets",async()=>{
    api.POST.mockResolvedValue({data:wire});api.PATCH.mockResolvedValue({data:wire});api.DELETE.mockResolvedValue({data:undefined});
    await createResource("Circuit Group Assignments",{name:"UI identity only",type:"circuit_group_assignment",groupId,member:{objectType:"circuits.circuit",objectId:memberId,label:"Do not send"},priority:"primary",tags:[tagId]});
    expect(api.POST).toHaveBeenCalledWith("/api/v1/inventory/circuit-group-assignments",{body:{group_id:groupId,member:{object_type:"circuits.circuit",object_id:memberId},priority:"primary",tags:[tagId]}});
    await updateResource("Circuit Group Assignments",id,{priority:null,tags:[]});
    expect(api.PATCH).toHaveBeenLastCalledWith("/api/v1/inventory/circuit-group-assignments/{id}",{params:{path:{id}},body:{priority:null,tags:[]}});
    await updateResource("Circuit Group Assignments",id,{customFields:{plain:"kept"}});
    expect(api.PATCH).toHaveBeenLastCalledWith("/api/v1/inventory/circuit-group-assignments/{id}",{params:{path:{id}},body:{custom_fields:{plain:"kept"}}});
    await deleteResource("Circuit Group Assignments",id);
    expect(api.DELETE).toHaveBeenCalledWith("/api/v1/inventory/circuit-group-assignments/{id}",{params:{path:{id}}});
  });
  it("rejects unsupported target kinds before a request and propagates denied writes",async()=>{
    await expect(updateResource("Circuit Group Assignments",id,{member:{objectType:"dcim.device",objectId:memberId,label:null}})).rejects.toThrow("Unsupported circuit group member type");
    expect(api.PATCH).not.toHaveBeenCalled();
    api.PATCH.mockResolvedValue({error:new Error("denied")});
    await expect(updateResource("Circuit Group Assignments",id,{priority:"secondary"})).rejects.toThrow("denied");
  });
  it("requires the group and typed member but never a phantom name, and observes type-only GFK changes",()=>{
    const resource={id,name:"WAN · Physical",type:"circuit_group_assignment",groupId,member:{objectType:"circuits.circuit",objectId:memberId,label:"Physical"},priority:"primary",tags:[tagId]};
    const initial=buildEntityDraft("Circuit Group Assignments",resource);
    expect(validateEntityDraft("Circuit Group Assignments",{...initial,name:""},key=>key)).toEqual({});
    expect(validateEntityDraft("Circuit Group Assignments",{...initial,groupId:""},key=>key)).toHaveProperty("groupId");
    const changed={...initial,[objectRefTypeKey("member")]:"circuits.virtualcircuit"};
    expect(draftToResourceUpdate("Circuit Group Assignments",changed,"Circuit Group Assignment",resource,initial)).toEqual({id,member:{objectType:"circuits.virtualcircuit",objectId:memberId,label:null}});
  });
  it("renders the existing polymorphic and Tag pickers without primary-model fields",()=>{
    function Form(){
      const resource={id,name:"WAN · Physical",type:"circuit_group_assignment",groupId,member:{objectType:"circuits.circuit",objectId:memberId,label:"Physical"},tags:[tagId]};
      const initial=buildEntityDraft("Circuit Group Assignments",resource);const [draft,setDraft]=useState(initial);
      return <><EntityFormBody entity="Circuit Group Assignments" draft={draft} onField={(key,value)=>setDraft(previous=>({...previous,[key]:value}))}
        loadOptions={entity=>entity==="Circuit Groups"?[{id:groupId,name:"WAN"}]:entity==="Circuits"?[{id:memberId,name:"Physical"}]:entity==="Tags"?[{id:tagId,name:"Core",objectTypes:[]}]:[]} onRequestCreate={()=>undefined}/>
        <output data-testid="assignment-payload">{JSON.stringify(draftToResourceUpdate("Circuit Group Assignments",draft,"Circuit Group Assignment",resource,initial))}</output></>;
    }
    render(<AllPermissions><Form/></AllPermissions>);
    for(const name of ["Name","Description","Comments"])expect(screen.queryByRole("textbox",{name})).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox",{name:"Owner"})).not.toBeInTheDocument();
    expect(screen.getByRole("combobox",{name:/Circuit group/})).toHaveTextContent("WAN");
    expect(screen.getByRole("combobox",{name:"Member"})).toHaveTextContent("Physical");
    expect(screen.getByTestId("assignment-payload")).toHaveTextContent(JSON.stringify({id}));
    fireEvent.click(screen.getByRole("button",{name:"Remove Core"}));
    expect(screen.getByTestId("assignment-payload")).toHaveTextContent(JSON.stringify({id,tags:[]}));
  });
});
