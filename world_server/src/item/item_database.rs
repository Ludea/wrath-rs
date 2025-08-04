use wow_world_messages::{wrath::UpdateItemBuilder, Guid};
use wrath_realm_db::item_instance::DBItemInstance;

use super::Item;

impl From<&DBItemInstance> for Item {
    fn from(value: &DBItemInstance) -> Self {
        //TODO: this object guid is not really correct, but guid format is not implemented yet
        //TODO: other attributes here need to be read from DB as well
        Item {
            update_state: UpdateItemBuilder::new()
                .set_object_guid(((value.character_id as u64) << (32 + value.slot_id as u64)).into())
                .set_object_entry(value.item.try_into().unwrap())
                .set_object_scale_x(1.0)
                .set_item_owner(Guid::new(value.character_id as u64))
                .set_item_contained(Guid::new(value.character_id as u64))
                .set_item_stack_count(1)
                .set_item_durability(100)
                .set_item_maxdurability(100)
                .finalize(),
        }
    }
}
