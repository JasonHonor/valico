use rustc_serialize::json::{self, ToJson};
use url;

use mutable_json::{MutableJson};
use super::super::json_schema;
use super::helpers;
use super::param;
use super::coercers;
use super::validators;
use super::errors;

pub struct Builder {
    requires: Vec<param::Param>,
    optional: Vec<param::Param>,
    validators: validators::Validators,
    schema_builder: Option<Box<Fn(&mut json_schema::Builder) + Send + Sync>>,
    schema_id: Option<url::Url>
}

unsafe impl Send for Builder { }

impl Builder {

    pub fn new() -> Builder {
        Builder {
            requires: vec![],
            optional: vec![],
            validators: vec![],
            schema_builder: None,
            schema_id: None
        }
    }

    pub fn build<F>(rules: F) -> Builder where F: FnOnce(&mut Builder) {
        let mut builder = Builder::new();
        rules(&mut builder);

        builder
    }

    pub fn get_required(&self) -> &Vec<param::Param> {
        return &self.requires;
    }

    pub fn get_optional(&self) -> &Vec<param::Param> {
        return &self.optional;
    }

    pub fn get_validators(&self) -> &validators::Validators {
        return &self.validators;
    }

    pub fn req_defined(&mut self, name: &str) {
        let params = param::Param::new(name);
        self.requires.push(params);
    }

    pub fn req_typed(&mut self, name: &str, coercer: Box<coercers::Coercer + Send + Sync>) {
        let params = param::Param::new_with_coercer(name, coercer);
        self.requires.push(params);
    }

    pub fn req_nested<F>(&mut self, name: &str, coercer: Box<coercers::Coercer + Send + Sync>, nest_def: F) where F: FnOnce(&mut Builder) {
        let nest_builder = Builder::build(nest_def);
        let params = param::Param::new_with_nest(name, coercer, nest_builder);
        self.requires.push(params);
    }

    pub fn req<F>(&mut self, name: &str, param_builder: F) where F: FnOnce(&mut param::Param) {
        let params = param::Param::build(name, param_builder);
        self.requires.push(params);
    }

    pub fn opt_defined(&mut self, name: &str) {
        let params = param::Param::new(name);
        self.optional.push(params);
    }

    pub fn opt_typed(&mut self, name: &str, coercer: Box<coercers::Coercer + Send + Sync>) {
        let params = param::Param::new_with_coercer(name, coercer);
        self.optional.push(params);
    }

    pub fn opt_nested<F>(&mut self, name: &str, coercer: Box<coercers::Coercer + Send + Sync>, nest_def: F) where F: FnOnce(&mut Builder) {
        let nest_builder = Builder::build(nest_def);
        let params = param::Param::new_with_nest(name, coercer, nest_builder);
        self.optional.push(params);
    }

    pub fn opt<F>(&mut self, name: &str, param_builder: F) where F: FnOnce(&mut param::Param) {
        let params = param::Param::build(name, param_builder);
        self.optional.push(params);
    }

    pub fn validate(&mut self, validator: Box<validators::Validator + 'static + Send + Sync>) {
        self.validators.push(validator);
    }

    pub fn validate_with<F>(&mut self, validator: F) where F: Fn(&json::Json, &str, bool) -> validators::ValidatorResult + Send+Sync {
        self.validators.push(Box::new(validator));
    }

    pub fn mutually_exclusive(&mut self, params: &[&str]) {
        let validator = Box::new(validators::MutuallyExclusive::new(params));
        self.validators.push(validator);
    }

    pub fn exactly_one_of(&mut self, params: &[&str]) {
        let validator = Box::new(validators::ExactlyOneOf::new(params));
        self.validators.push(validator);
    }

    pub fn at_least_one_of(&mut self, params: &[&str]) {
        let validator = Box::new(validators::AtLeastOneOf::new(params));
        self.validators.push(validator);
    }

    pub fn schema<F>(&mut self, build: F) where F: Fn(&mut json_schema::Builder,) + Send + Sync {
        self.schema_builder = Some(Box::new(build));
    }

    pub fn build_schemes(&mut self, scope: &mut json_schema::Scope) -> Result<(), json_schema::SchemaError> {
        for param in self.requires.iter_mut().chain(self.optional.iter_mut()) {
            if param.schema_builder.is_some() {
                let json_schema = json_schema::builder::schema_box(param.schema_builder.take().unwrap());
                let id = try!(scope.compile(json_schema.to_json()));
                param.schema_id = Some(id);
            }

            if param.nest.is_some() {
                try!(param.nest.as_mut().unwrap().build_schemes(scope));
            }
        }

        if self.schema_builder.is_some() {
            let json_schema = json_schema::builder::schema_box(self.schema_builder.take().unwrap());
            let id = try!(scope.compile(json_schema.to_json()));
            self.schema_id = Some(id);
        }

        Ok(())
    }

    pub fn process(&self, val: &mut json::Json, scope: &Option<&json_schema::Scope>) -> json_schema::ValidationState {
        self.process_nest(val, "", scope)
    }

    pub fn process_nest(&self, val: &mut json::Json, path: &str, scope: &Option<&json_schema::Scope>) -> json_schema::ValidationState {
        let mut state = if val.is_array() {
            let mut state = json_schema::ValidationState::new();
            let array = val.as_array_mut().unwrap();
            for (idx, item) in array.iter_mut().enumerate() {
                let item_path = [path, idx.to_string().as_slice()].connect("/");
                if item.is_object() {
                    let mut process_state = self.process_object(item, item_path.as_slice(), scope);
                    state.append(&mut process_state);
                } else {
                    state.errors.push(
                        Box::new(errors::WrongType {
                            path: item_path.to_string(),
                            detail: "List value is not and object".to_string()
                        })
                    )
                }
            }

            state
        } else if val.is_object() {
            self.process_object(val, path, scope)
        } else {
            let mut state = json_schema::ValidationState::new();
            state.errors.push(
                Box::new(errors::WrongType {
                    path: path.to_string(),
                    detail: "Value is not an object or an array".to_string()
                }) as Box<super::super::common::error::ValicoError>
            );

            state
        };

        if self.schema_id.is_some() && scope.is_some() {
            let id = self.schema_id.as_ref().unwrap();
            let schema = scope.as_ref().unwrap().resolve(id);
            match schema {
                Some(schema) => state.append(&mut schema.validate_in(val, path)),
                None => state.missing.push(id.clone())
            }
        }

        state
    }

    fn process_object(&self, val: &mut json::Json, path: &str, scope: &Option<&json_schema::Scope>) -> json_schema::ValidationState  {
        
        let mut state = json_schema::ValidationState::new();

        {
            let object = val.as_object_mut().expect("We expect object here");
            for param in self.requires.iter() {
                let ref name = param.name;
                let present = helpers::has_value(object, name);
                let param_path = [path, name.as_slice()].connect("/");
                if present {
                    let mut process_result = param.process(object.get_mut(name).unwrap(), param_path.as_slice(), scope);
                    match process_result.value  {
                        Some(new_value) => { object.insert(name.clone(), new_value); },
                        None => ()
                    }

                    state.append(&mut process_result.state);
                } else {
                    state.errors.push(Box::new(errors::Required {
                        path: param_path.clone()
                    }))
                }
            }

            for param in self.optional.iter() {
                let ref name = param.name;
                let present = helpers::has_value(object, name);
                let param_path = [path, name.as_slice()].connect("/");
                if present {
                    let mut process_result = param.process(object.get_mut(name).unwrap(), param_path.as_slice(), scope);
                    match process_result.value  {
                        Some(new_value) => { object.insert(name.clone(), new_value); },
                        None => ()
                    }

                    state.append(&mut process_result.state);
                }
            }
        }

        let path = if path == "" {
            "/"
        } else {
            path
        };

        for validator in self.validators.iter() {
            match validator.validate(val, path, true) {
                Ok(()) => (),
                Err(mut err) => {
                    state.errors.append(&mut err);
                }
            };
        }

        {
            if state.is_valid() {
                let object = val.as_object_mut().expect("We expect object here");

                // second pass we need to validate without default values in optionals
                for param in self.optional.iter() {
                    let ref name = param.name;
                    let present = helpers::has_value(object, name);
                    if !present {
                        match param.default.as_ref() {
                            Some(val) => { object.insert(name.clone(), val.clone()); },
                            None => ()
                        };
                    }
                }
            }
        }

        state
    }
}


