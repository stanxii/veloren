use crate::{
    assets::{self, AssetExt, AssetHandle},
    comp::{
        inventory::slot::InvSlotId,
        item::{modular, tool::AbilityMap, ItemDef, ItemTag, MaterialStatManifest},
        Inventory, Item,
    },
    terrain::SpriteKind,
};
use hashbrown::HashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RecipeInput {
    Item(Arc<ItemDef>),
    Tag(ItemTag),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Recipe {
    pub output: (Arc<ItemDef>, u32),
    pub inputs: Vec<(RecipeInput, u32)>,
    pub craft_sprite: Option<SpriteKind>,
}

#[allow(clippy::type_complexity)]
impl Recipe {
    /// Perform a recipe, returning a list of missing items on failure
    pub fn craft_simple(
        &self,
        inv: &mut Inventory,
        // Vec tying an input to a slot
        slots: Vec<(u32, InvSlotId)>,
        ability_map: &AbilityMap,
        msm: &MaterialStatManifest,
    ) -> Result<Vec<Item>, Vec<(&RecipeInput, u32)>> {
        let mut slot_claims = HashMap::new();
        let mut unsatisfied_requirements = Vec::new();

        // Checks each input against slots in the inventory. If the slots contain an
        // item that fulfills the need of the input, marks some of the item as claimed
        // up to quantity needed for the crafting input. If the item either
        // cannot be used, or there is insufficient quantity, adds input and
        // number of materials needed to unsatisfied requirements.
        self.inputs
            .iter()
            .enumerate()
            .for_each(|(i, (input, mut required))| {
                // Check used for recipes that have an input that is not consumed, e.g.
                // craftsman hammer
                let mut contains_any = false;
                // Gets all slots provided for this input by the frontend
                let input_slots = slots
                    .iter()
                    .filter_map(|(j, slot)| if i as u32 == *j { Some(slot) } else { None });
                // Goes through each slot and marks some amount from each slot as claimed
                for slot in input_slots {
                    // Checks that the item in the slot can be used for the input
                    if let Some(item) = inv
                        .get(*slot)
                        .filter(|item| item.matches_recipe_input(input))
                    {
                        // Gets the number of items claimed from the slot, or sets to 0 if slot has
                        // not been claimed by another input yet
                        let claimed = slot_claims.entry(*slot).or_insert(0);
                        let available = item.amount().saturating_sub(*claimed);
                        let provided = available.min(required);
                        required -= provided;
                        *claimed += provided;
                        contains_any = true;
                    }
                }
                // If there were not sufficient items to cover requirement between all provided
                // slots, or if non-consumed item was not present, mark input as not satisfied
                if required > 0 || !contains_any {
                    unsatisfied_requirements.push((input, required));
                }
            });

        // If there are no unsatisfied requirements, create the items produced by the
        // recipe in the necessary quantity and remove the items that the recipe
        // consumes
        if unsatisfied_requirements.is_empty() {
            for (slot, to_remove) in slot_claims.iter() {
                for _ in 0..*to_remove {
                    let _ = inv
                        .take(*slot, ability_map, msm)
                        .expect("Expected item to exist in the inventory");
                }
            }
            let (item_def, quantity) = &self.output;
            let crafted_item = Item::new_from_item_def(Arc::clone(item_def), &[], ability_map, msm);
            let mut crafted_items = Vec::with_capacity(*quantity as usize);
            for _ in 0..*quantity {
                crafted_items.push(crafted_item.duplicate(ability_map, msm));
            }
            Ok(crafted_items)
        } else {
            Err(unsatisfied_requirements)
        }
    }

    pub fn inputs(&self) -> impl ExactSizeIterator<Item = (&RecipeInput, u32)> {
        self.inputs
            .iter()
            .map(|(item_def, amount)| (item_def, *amount))
    }

    /// Determine whether the inventory contains the ingredients for a recipe.
    /// If it does, return a vec of  inventory slots that contain the
    /// ingredients needed, whose positions correspond to particular recipe
    /// inputs. If items are missing, return the missing items, and how many
    /// are missing.
    pub fn inventory_contains_ingredients<'a>(
        &self,
        inv: &'a Inventory,
    ) -> Result<Vec<(u32, InvSlotId)>, Vec<(&RecipeInput, u32)>> {
        // Hashmap tracking the quantity that needs to be removed from each slot (so
        // that it doesn't think a slot can provide more items than it contains)
        let mut slot_claims = HashMap::<InvSlotId, u32>::new();
        // Important to be a vec and to remain separate from slot_claims as it must
        // remain ordered, unlike the hashmap
        let mut slots = Vec::<(u32, InvSlotId)>::new();
        // The inputs to a recipe that have missing items, and the amount missing
        let mut missing = Vec::<(&RecipeInput, u32)>::new();

        for (i, (input, mut needed)) in self.inputs().enumerate() {
            let mut contains_any = false;
            // Checks through every slot, filtering to only those that contain items that
            // can satisfy the input
            for (inv_slot_id, slot) in inv.slots_with_id() {
                if let Some(item) = slot
                    .as_ref()
                    .filter(|item| item.matches_recipe_input(&*input))
                {
                    let claim = slot_claims.entry(inv_slot_id).or_insert(0);
                    slots.push((i as u32, inv_slot_id));
                    let can_claim = (item.amount().saturating_sub(*claim)).min(needed);
                    *claim += can_claim;
                    needed -= can_claim;
                    contains_any = true;
                }
            }

            if needed > 0 || !contains_any {
                missing.push((input, needed));
            }
        }

        if missing.is_empty() {
            Ok(slots)
        } else {
            Err(missing)
        }
    }
}

pub enum SalvageError {
    NotSalvageable,
}

pub fn try_salvage(
    inv: &mut Inventory,
    slot: InvSlotId,
    ability_map: &AbilityMap,
    msm: &MaterialStatManifest,
) -> Result<Vec<Item>, SalvageError> {
    if inv.get(slot).map_or(false, |item| item.is_salvageable()) {
        let salvage_item = inv.get(slot).expect("Expected item to exist in inventory");
        let salvage_output: Vec<_> = salvage_item
            .salvage_output()
            .map(Item::new_from_asset_expect)
            .collect();
        if salvage_output.is_empty() {
            // If no output items, assume salvaging was a failure
            // TODO: If we ever change salvaging to have a percent chance, remove the check
            // of outputs being empty (requires assets to exist for rock and wood materials
            // so that salvaging doesn't silently fail)
            Err(SalvageError::NotSalvageable)
        } else {
            // Remove item that is being salvaged
            let _ = inv
                .take(slot, ability_map, msm)
                .expect("Expected item to exist in inventory");
            // Return the salvaging output
            Ok(salvage_output)
        }
    } else {
        Err(SalvageError::NotSalvageable)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecipeBook {
    recipes: HashMap<String, Recipe>,
}

impl RecipeBook {
    pub fn get(&self, recipe: &str) -> Option<&Recipe> { self.recipes.get(recipe) }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&String, &Recipe)> { self.recipes.iter() }

    pub fn get_available(&self, inv: &Inventory) -> Vec<(String, Recipe)> {
        self.recipes
            .iter()
            .filter(|(_, recipe)| recipe.inventory_contains_ingredients(inv).is_ok())
            .map(|(name, recipe)| (name.clone(), recipe.clone()))
            .collect()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RawRecipeInput {
    Item(String),
    Tag(ItemTag),
}

#[derive(Clone, Deserialize)]
pub(crate) struct RawRecipe {
    pub(crate) output: (String, u32),
    pub(crate) inputs: Vec<(RawRecipeInput, u32)>,
    pub(crate) craft_sprite: Option<SpriteKind>,
}

#[derive(Clone, Deserialize)]
#[serde(transparent)]
pub(crate) struct RawRecipeBook(pub(crate) HashMap<String, RawRecipe>);

impl assets::Asset for RawRecipeBook {
    type Loader = assets::RonLoader;

    const EXTENSION: &'static str = "ron";
}

impl assets::Compound for RecipeBook {
    fn load<S: assets::source::Source>(
        cache: &assets::AssetCache<S>,
        specifier: &str,
    ) -> Result<Self, assets::Error> {
        #[inline]
        fn load_item_def(spec: &(String, u32)) -> Result<(Arc<ItemDef>, u32), assets::Error> {
            let def = Arc::<ItemDef>::load_cloned(&spec.0)?;
            Ok((def, spec.1))
        }

        #[inline]
        fn load_recipe_input(
            spec: &(RawRecipeInput, u32),
        ) -> Result<(RecipeInput, u32), assets::Error> {
            let def = match &spec.0 {
                RawRecipeInput::Item(name) => RecipeInput::Item(Arc::<ItemDef>::load_cloned(name)?),
                RawRecipeInput::Tag(tag) => RecipeInput::Tag(*tag),
            };
            Ok((def, spec.1))
        }

        let mut raw = cache.load::<RawRecipeBook>(specifier)?.read().clone();

        // Avoid showing purple-question-box recipes until the assets are added
        // (the `if false` is needed because commenting out the call will add a warning
        // that there are no other uses of append_modular_recipes)
        if false {
            modular::append_modular_recipes(&mut raw);
        }

        let recipes = raw
            .0
            .iter()
            .map(
                |(
                    name,
                    RawRecipe {
                        output,
                        inputs,
                        craft_sprite,
                    },
                )| {
                    let inputs = inputs
                        .iter()
                        .map(load_recipe_input)
                        .collect::<Result<Vec<_>, _>>()?;
                    let output = load_item_def(output)?;
                    Ok((name.clone(), Recipe {
                        output,
                        inputs,
                        craft_sprite: *craft_sprite,
                    }))
                },
            )
            .collect::<Result<_, assets::Error>>()?;

        Ok(RecipeBook { recipes })
    }
}

pub fn default_recipe_book() -> AssetHandle<RecipeBook> {
    RecipeBook::load_expect("common.recipe_book")
}
